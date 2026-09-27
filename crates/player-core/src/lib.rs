use std::fs::File;
use std::io::BufReader;
use std::path::Path;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use rodio::{Decoder, OutputStream, OutputStreamHandle, Sink, Source};
mod handle;
pub use handle::{PlayerHandle, Status};

#[derive(thiserror::Error, Debug)]
pub enum PlayerError {
    #[error("failed to open audio output device: {0}")]
    Device(#[from] rodio::StreamError),
    #[error("failed to create playback sink: {0}")]
    Sink(#[from] rodio::PlayError),
    #[error("failed to read file: {0}")]
    Io(#[from] std::io::Error),
    #[error("failed to decode audio: {0}")]
    Decode(#[from] rodio::decoder::DecoderError),
    #[error("seek failed: {0}")]
    Seek(String),
    #[error("no track loaded")]
    NoTrack,
    #[error("audio command timed out")]
    CommandTimeout,
}

const CAVA_BARS_PER_CHANNEL: usize = 24;
const CAVA_FFT_SIZE: usize = 4096;
const CAVA_BASS_FFT_SIZE: usize = CAVA_FFT_SIZE * 2;
const CAVA_WINDOW_FRAMES: usize = CAVA_BASS_FFT_SIZE;
const CAVA_SAMPLE_CAPACITY: usize = CAVA_WINDOW_FRAMES * 2;

/// A lock-free rolling PCM window used to calculate CAVA-compatible bands.
/// Only the 48 compact band values leave the native player.
struct SpectrumBuffer {
    samples: [AtomicU32; CAVA_SAMPLE_CAPACITY],
    write_index: AtomicUsize,
    channels: AtomicUsize,
}

impl SpectrumBuffer {
    fn new() -> Self {
        Self {
            samples: std::array::from_fn(|_| AtomicU32::new(0)),
            write_index: AtomicUsize::new(0),
            channels: AtomicUsize::new(2),
        }
    }

    fn push(&self, sample: f32) {
        let index = self.write_index.fetch_add(1, Ordering::Relaxed) % CAVA_SAMPLE_CAPACITY;
        self.samples[index].store(sample.to_bits(), Ordering::Relaxed);
    }

    fn clear(&self) {
        self.write_index.store(0, Ordering::Relaxed);
        for sample in &self.samples {
            sample.store(0, Ordering::Relaxed);
        }
    }

    fn set_channels(&self, channels: u16) {
        self.channels.store(usize::from(channels.max(1)), Ordering::Relaxed);
    }

    fn spectrum(&self, sample_rate: u32, volume: f32) -> Vec<f32> {
        let channels = self.channels.load(Ordering::Relaxed).clamp(1, 8);
        let end = self.write_index.load(Ordering::Relaxed);
        let end_frame = end / channels;
        let start_frame = end_frame.saturating_sub(CAVA_WINDOW_FRAMES);
        let available_frames = end_frame - start_frame;
        let missing_frames = CAVA_WINDOW_FRAMES - available_frames;
        let mut left = vec![0.0_f32; CAVA_WINDOW_FRAMES];
        let mut right = vec![0.0_f32; CAVA_WINDOW_FRAMES];

        for frame in 0..available_frames {
            let absolute_frame = start_frame + frame;
            let left_index = (absolute_frame * channels) % CAVA_SAMPLE_CAPACITY;
            let right_index = if channels > 1 { (left_index + 1) % CAVA_SAMPLE_CAPACITY } else { left_index };
            left[missing_frames + frame] =
                f32::from_bits(self.samples[left_index].load(Ordering::Relaxed)) * volume;
            right[missing_frames + frame] =
                f32::from_bits(self.samples[right_index].load(Ordering::Relaxed)) * volume;
        }

        let left_bars = cava_bands(&left, sample_rate);
        let right_bars = cava_bands(&right, sample_rate);
        left_bars.into_iter().chain(right_bars).collect()
    }
}

fn cava_bands(samples: &[f32], sample_rate: u32) -> Vec<f32> {
    let nyquist = sample_rate as f64 / 2.0;
    let low_cutoff = 50.0_f64.min(nyquist * 0.5);
    let high_cutoff = 10_000.0_f64.min(nyquist);
    if high_cutoff <= low_cutoff || samples.len() < CAVA_FFT_SIZE {
        return vec![0.0; CAVA_BARS_PER_CHANNEL];
    }

    let main_start = samples.len() - CAVA_FFT_SIZE;
    let main_spectrum = fft_magnitudes(&samples[main_start..]);
    let bass_spectrum = fft_magnitudes(samples);
    let ratio = high_cutoff / low_cutoff;
    let mut output = Vec::with_capacity(CAVA_BARS_PER_CHANNEL);

    for bar in 0..CAVA_BARS_PER_CHANNEL {
        let lower = low_cutoff * ratio.powf(bar as f64 / CAVA_BARS_PER_CHANNEL as f64);
        let upper = low_cutoff * ratio.powf((bar + 1) as f64 / CAVA_BARS_PER_CHANNEL as f64);
        let fft_size = if lower < 100.0 { CAVA_BASS_FFT_SIZE } else { CAVA_FFT_SIZE };
        let spectrum = if fft_size == CAVA_BASS_FFT_SIZE { &bass_spectrum } else { &main_spectrum };
        let lower_bin = (if fft_size == CAVA_BASS_FFT_SIZE {
            (lower * fft_size as f64 / sample_rate as f64).floor() as usize
        } else {
            (lower * fft_size as f64 / sample_rate as f64).ceil() as usize
        }).min(spectrum.len().saturating_sub(1));
        let upper_bin = ((upper * fft_size as f64 / sample_rate as f64).ceil() as usize)
            .saturating_sub(1)
            .min(spectrum.len().saturating_sub(1))
            .max(lower_bin);
        let magnitude_sum: f64 = spectrum[lower_bin..=upper_bin].iter().sum();
        let bin_count = (upper_bin - lower_bin + 1) as f64;

        // CAVA's linear scaling: equalize for band width and frequency before
        // applying its 16-bit PCM scale and adaptive sensitivity in the UI.
        let equalizer = upper.powf(0.85) / (2.0_f64.powi(28) * (fft_size as f64).log2() * bin_count);
        output.push((magnitude_sum * equalizer) as f32);
    }

    output
}

fn fft_magnitudes(samples: &[f32]) -> Vec<f64> {
    let n = samples.len();
    let mut real = vec![0.0_f64; n];
    let mut imag = vec![0.0_f64; n];

    for (i, sample) in samples.iter().enumerate() {
        let window = 0.5 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / (n - 1) as f64).cos();
        real[i] = f64::from(*sample) * 32_768.0 * window;
    }

    let mut j = 0;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j ^= bit;
        if i < j {
            real.swap(i, j);
            imag.swap(i, j);
        }
    }

    let mut len = 2;
    while len <= n {
        let angle = -2.0 * std::f64::consts::PI / len as f64;
        let (step_real, step_imag) = (angle.cos(), angle.sin());
        for start in (0..n).step_by(len) {
            let (mut wr, mut wi) = (1.0, 0.0);
            for offset in 0..len / 2 {
                let even = start + offset;
                let odd = even + len / 2;
                let (vr, vi) = (real[odd] * wr - imag[odd] * wi, real[odd] * wi + imag[odd] * wr);
                let (ur, ui) = (real[even], imag[even]);
                real[even] = ur + vr;
                imag[even] = ui + vi;
                real[odd] = ur - vr;
                imag[odd] = ui - vi;
                (wr, wi) = (wr * step_real - wi * step_imag, wr * step_imag + wi * step_real);
            }
        }
        len <<= 1;
    }

    real.iter().zip(&imag).take(n / 2 + 1).map(|(re, im)| re.hypot(*im)).collect()
}

/// Passes decoded samples to Rodio unchanged while retaining a short stereo
/// history for the CAVA-style spectrum analyzer.
struct SpectrumSource<S> {
    inner: S,
    spectrum_buffer: Arc<SpectrumBuffer>,
    input_channels: usize,
    sample_index: usize,
}

impl<S: Source<Item = f32>> SpectrumSource<S> {
    fn new(inner: S, spectrum_buffer: Arc<SpectrumBuffer>) -> Self {
        let input_channels = usize::from(inner.channels().max(1));
        spectrum_buffer.set_channels(input_channels.min(2) as u16);
        Self { inner, spectrum_buffer, input_channels, sample_index: 0 }
    }
}

impl<S> Iterator for SpectrumSource<S>
where
    S: Source<Item = f32>,
{
    type Item = f32;

    fn next(&mut self) -> Option<Self::Item> {
        let sample = self.inner.next()?;
        if self.sample_index % self.input_channels < 2 {
            self.spectrum_buffer.push(sample);
        }
        self.sample_index += 1;
        Some(sample)
    }
}

impl<S> Source for SpectrumSource<S>
where
    S: Source<Item = f32>,
{
    fn current_frame_len(&self) -> Option<usize> {
        self.inner.current_frame_len()
    }

    fn channels(&self) -> u16 {
        self.inner.channels()
    }

    fn sample_rate(&self) -> u32 {
        self.inner.sample_rate()
    }

    fn total_duration(&self) -> Option<Duration> {
        self.inner.total_duration()
    }

    fn try_seek(&mut self, position: Duration) -> Result<(), rodio::source::SeekError> {
        self.inner.try_seek(position)
    }
}

pub struct Player {
    _stream: OutputStream, // must stay alive for the whole player's lifetime
    stream_handle: OutputStreamHandle,
    sink: Option<Sink>,
    total_duration: Option<Duration>,
    position_base: Duration,        // committed position as of the last play/pause/seek
    playing_since: Option<Instant>,
    current_path: Option<PathBuf>,
    volume: f32,
    spectrum_buffer: Arc<SpectrumBuffer>,
    sample_rate: u32,
}

impl Player {
    pub fn new() -> Result<Self, PlayerError> {
        let (stream, stream_handle) = OutputStream::try_default()?;
        Ok(Self { 
            _stream: stream, 
            stream_handle, 
            sink: None,
            total_duration: None,
            position_base: Duration::ZERO,
            playing_since: None,
            current_path: None,
            volume: 1.0,
            spectrum_buffer: Arc::new(SpectrumBuffer::new()),
            sample_rate: 44_100,
        })
    }

    pub fn load(&mut self, path: &Path) -> Result<(), PlayerError> {
        let file = File::open(path)?;
        let source = Decoder::new(BufReader::new(file))?;
        self.total_duration = source.total_duration();
        self.sample_rate = source.sample_rate();
        self.spectrum_buffer.clear();
        let source = SpectrumSource::new(source.convert_samples::<f32>(), self.spectrum_buffer.clone());

        let sink = Sink::try_new(&self.stream_handle)?;
        sink.append(source);
        sink.set_volume(self.volume);
        sink.pause(); // load() shouldn't auto-play; caller decides
        self.sink = Some(sink);
        self.position_base = Duration::ZERO;
        self.playing_since = None;
        self.current_path = Some(path.to_owned()); 
        Ok(())
    }

    pub fn play(&mut self) {
        if let Some(sink) = &self.sink {
            sink.play();
            if self.playing_since.is_none() {
                self.playing_since = Some(Instant::now());
            }
        }
    }

    pub fn pause(&mut self) {
        if let Some(sink) = &self.sink {
            sink.pause();
        }
        self.spectrum_buffer.clear();
        if let Some(since) = self.playing_since.take() {
            self.position_base += since.elapsed();
        }
    }

    pub fn toggle(&mut self) -> Result<(), PlayerError> {
        if self.sink.is_none() {
            return Err(PlayerError::NoTrack);
        }

        if self.is_playing() {
            self.pause();
        } else {
            // A completed sink has no source left to resume. Reloading makes
            // Play work again after the last track in a queue has finished.
            if self.finished() {
                let path = self.current_path.clone().ok_or(PlayerError::NoTrack)?;
                self.load(&path)?;
            }
            self.play();
        }

        Ok(())
    }

    pub fn is_playing(&self) -> bool {
        match &self.sink {
            Some(sink) if !sink.empty() => !sink.is_paused(),
            _ => false,
        }
    }

    pub fn position(&self) -> Duration {
        let raw = match self.playing_since {
            Some(since) => self.position_base + since.elapsed(),
            None => self.position_base,
        };
        match self.total_duration {
            Some(total) => raw.min(total),
            None => raw,
        }
    }

    pub fn total_duration(&self) -> Option<Duration> {
        self.total_duration
    }

    pub fn seek(&mut self, position: Duration) -> Result<(), PlayerError> {
        if self.finished() {
            let path = self.current_path.clone().ok_or(PlayerError::NoTrack)?;
            self.load(&path)?; // fresh sink with the source re-queued
        }

        let sink = self.sink.as_ref().ok_or(PlayerError::NoTrack)?;
        sink.try_seek(position).map_err(|e| PlayerError::Seek(e.to_string()))?;
        self.position_base = position;

        let was_playing = self.playing_since.take().is_some();

        if was_playing {
            self.play(); // resumes real playback, not just the displayed clock
        }

        Ok(())
    }

    pub fn volume(&self) -> f32 {
        self.volume
    }

    pub fn spectrum(&self) -> Vec<f32> {
        if self.is_playing() {
            self.spectrum_buffer.spectrum(self.sample_rate, self.volume)
        } else {
            vec![0.0; CAVA_BARS_PER_CHANNEL * 2]
        }
    }

    pub fn set_volume(&mut self, vol: f32) {
        self.volume = vol.clamp(0.0, 1.0);
        if let Some(sink) = &self.sink {
            sink.set_volume(self.volume);
        }
    }

    pub fn finished(&self) -> bool {
        self.sink.as_ref().map(|s| s.empty()).unwrap_or(false)
    }
}
