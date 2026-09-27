const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const { open } = window.__TAURI__.dialog;

const playPauseBtn = document.getElementById("play-pause-btn");
const playPauseLabel = playPauseBtn.querySelector("span");
const seekBar = document.getElementById("seek-bar");
const timeDisplay = document.getElementById("time-display");
const volumeSlider = document.getElementById("volume");
const nowPlayingEl = document.getElementById("now-playing");
const trackListEl = document.getElementById("track-list");
const liveIndicator = document.getElementById("status-live");
const hiraganaEl = document.querySelector(".term__hiragana");
const clockEl = document.getElementById("status-clock");
const canvas = document.getElementById("oscilloscope");
const scopeCtx = canvas.getContext("2d");


let lastOpenedPath = null;
let lastKnownStatus = null;
let libraryTracks = [];
let isDraggingSeek = false;
let trackLoaded = false;
let isPlaying = false;
let pendingToggles = 0;
let statusPollInFlight = false;
let playbackRevision = 0;
let pendingVolume = Number(volumeSlider.value);
let volumeTimer = null;
let barsFramePending = false;
let lastStatusPollAt = 0;
let lastMprisTitle = null;
let mprisUnavailable = false;

// ── protocol drone (real Web Audio synth + analyser) ──
let audioCtx = null;
let droneOscillators = [];
let droneGain = null;
let droneAnalyser = null;
let droneActive = false;

function startDrone() {
  if (!audioCtx) audioCtx = new (window.AudioContext || window.webkitAudioContext)();
  const ctx = audioCtx;

  const osc1 = ctx.createOscillator();
  osc1.type = "sine";
  osc1.frequency.value = 55;
  const osc2 = ctx.createOscillator();
  osc2.type = "sine";
  osc2.frequency.value = 55 * 1.5;

  droneGain = ctx.createGain();
  droneGain.gain.value = 0.0001;
  droneAnalyser = ctx.createAnalyser();
  droneAnalyser.fftSize = 256;

  osc1.connect(droneGain);
  osc2.connect(droneGain);
  droneGain.connect(droneAnalyser);
  droneAnalyser.connect(ctx.destination);

  osc1.start();
  osc2.start();
  droneGain.gain.exponentialRampToValueAtTime(0.05, ctx.currentTime + 1.5);

  droneOscillators = [osc1, osc2];
  droneActive = true;
  scheduleBars();
}

function stopDrone() {
  if (!droneActive || !audioCtx) return;
  const ctx = audioCtx;
  droneGain.gain.exponentialRampToValueAtTime(0.0001, ctx.currentTime + 0.8);
  setTimeout(() => {
    droneOscillators.forEach((o) => { try { o.stop(); } catch (_) {} });
    droneOscillators = [];
  }, 900);
  droneActive = false;
  scheduleBars();
}

function setLiveIndicator(live) {
  liveIndicator.classList.toggle("is-live", live);
  hiraganaEl.classList.toggle("is-live", live);
}

function setPlayPauseLabel(label) {
  playPauseLabel.textContent = label;
}

// ── CAVA-style stereo spectrum ──
const BAR_COUNT = 48;
let targetBars = new Array(BAR_COUNT).fill(0);
const CAVA_NOISE_REDUCTION = 0.77;
const CAVA_FRAME_RATE = 60;
const CAVA_AUTOSENS = 1;
let cavaSensitivity = 1;
let cavaSensitivityInit = true;
let cavaPrevious = new Array(BAR_COUNT).fill(0);
let cavaPeak = new Array(BAR_COUNT).fill(0);
let cavaFall = new Array(BAR_COUNT).fill(0);
let cavaMemory = new Array(BAR_COUNT).fill(0);

function resizeCanvas() {
  const dpr = window.devicePixelRatio || 1;
  canvas.width = canvas.clientWidth * dpr;
  canvas.height = canvas.clientHeight * dpr;
  scheduleBars();
}
window.addEventListener("resize", resizeCanvas);

function computeSpectrumFromAnalyser(analyser) {
  const freqData = new Uint8Array(analyser.frequencyBinCount);
  analyser.getByteFrequencyData(freqData);
  const bars = new Array(BAR_COUNT).fill(0);
  for (let i = 0; i < BAR_COUNT; i++) {
    const fraction = (i % (BAR_COUNT / 2) + 1) / (BAR_COUNT / 2);
    const bin = Math.min(freqData.length - 1, Math.floor(fraction * fraction * freqData.length));
    bars[i] = freqData[bin] / 255;
  }
  return bars;
}

function scheduleBars() {
  if (barsFramePending) return;
  barsFramePending = true;
  requestAnimationFrame(drawBars);
}

function drawBars() {
  barsFramePending = false;
  const w = canvas.width, h = canvas.height;
  if (!w || !h) return;

  if (droneActive && droneAnalyser) targetBars = computeSpectrumFromAnalyser(droneAnalyser);
  const live = isPlaying || droneActive;

  scopeCtx.clearRect(0, 0, w, h);
  const barW = w / BAR_COUNT;
  const gap = barW * 0.22;
  const framerateMod = 66 / CAVA_FRAME_RATE;
  const gravityMod = Math.pow(framerateMod, 2.5) * 2 / CAVA_NOISE_REDUCTION;
  const integralMod = Math.pow(framerateMod, 0.1);
  let overshoot = false;
  let hasSignal = false;
  let hasTail = false;

  for (let i = 0; i < BAR_COUNT; i++) {
    // CAVA's stereo display mirrors the channels with bass toward the center.
    const channelBar = i < BAR_COUNT / 2 ? (BAR_COUNT / 2 - 1 - i) : (i - BAR_COUNT / 2);
    const targetIndex = i < BAR_COUNT / 2 ? channelBar : BAR_COUNT / 2 + channelBar;
    const input = Math.max(0, targetBars[targetIndex] ?? 0) * cavaSensitivity;
    hasSignal ||= input > 0.00001;
    let value = input;

    if (value < cavaPrevious[i] && CAVA_NOISE_REDUCTION > 0.1) {
      value = Math.max(0, cavaPeak[i] * (1 - cavaFall[i] * cavaFall[i] * gravityMod));
      cavaFall[i] += 0.028;
    } else {
      cavaPeak[i] = value;
      cavaFall[i] = 0;
    }
    cavaPrevious[i] = value;
    value = cavaMemory[i] * CAVA_NOISE_REDUCTION / integralMod + value;
    cavaMemory[i] = value;
    if (CAVA_AUTOSENS && value > 1) {
      overshoot = true;
      value = 1;
    }
    hasTail ||= value > 0.005;
    const barH = Math.max(1, Math.round(Math.min(1, value) * h));

    const x = i * barW + gap / 2;
    scopeCtx.fillStyle = live ? "#E3A857" : "rgba(199,204,198,0.15)";
    scopeCtx.fillRect(x, h - barH, barW - gap, barH);
  }

  if (CAVA_AUTOSENS) {
    if (overshoot) {
      cavaSensitivity *= 1 - (0.02 * framerateMod);
      cavaSensitivityInit = false;
    } else if (hasSignal) {
      cavaSensitivity *= 1 + (0.001 * framerateMod * CAVA_AUTOSENS);
      if (cavaSensitivityInit) cavaSensitivity *= 1 + 0.1 * framerateMod;
    }
  }

  if (live || hasTail) scheduleBars();
}
// ── clock ──
function updateClock() {
  clockEl.textContent = new Intl.DateTimeFormat(undefined, {
    hour: "numeric",
    minute: "2-digit",
    hour12: true,
  }).format(new Date());
}

function formatTime(seconds) {
  if (!Number.isFinite(seconds)) return "0:00";
  const m = Math.floor(seconds / 60);
  const s = Math.floor(seconds % 60).toString().padStart(2, "0");
  return `${m}:${s}`;
}

// ── library ──
async function openPath(path) {
  try {
    libraryTracks = await invoke("library_scan_paths", { paths: [path] });
    renderTrackList();
    lastOpenedPath = path;
    if (libraryTracks.length > 0) await playFromIndex(0);
    saveState();
  } catch (err) {
    nowPlayingEl.textContent = `error: ${err}`;
  }
}

function renderTrackList() {
  trackListEl.replaceChildren();
  libraryTracks.forEach((track, index) => {
    const li = document.createElement("li");
    li.dataset.index = String(index);
    const num = String(index + 1).padStart(2, "0");
    const indexEl = document.createElement("span");
    indexEl.className = "lib__index";
    indexEl.textContent = num;
    li.append(indexEl, document.createTextNode(`${track.title} — ${track.artist}`));
    li.addEventListener("click", () => playFromIndex(index));
    trackListEl.appendChild(li);
  });
}

async function playFromIndex(index, autoplay = true) {
  if (droneActive) stopDrone();
  const paths = libraryTracks.map((t) => t.path);
  await invoke("queue_load", { paths, startIndex: index, autoplay });
  trackLoaded = true;
  seekBar.disabled = false;
}

function saveState() {
  invoke("state_save", {
    lastPath: lastOpenedPath,
    lastIndex: lastKnownStatus?.queue_index ?? null,
    lastPositionSecs: lastKnownStatus?.position_secs ?? null,
    volume: lastKnownStatus?.volume ?? Number(volumeSlider.value),
  }).catch((err) => console.error("state save failed:", err));
}
setInterval(saveState, 5000);

listen("tauri://drag-drop", async (event) => {
  const paths = event.payload.paths;
  if (!paths || paths.length === 0) return;
  libraryTracks = await invoke("library_scan_paths", { paths });
  renderTrackList();
  if (paths.length === 1) lastOpenedPath = paths[0];
  if (libraryTracks.length > 0) await playFromIndex(0);
  saveState();
});

document.getElementById("open-file-btn").addEventListener("click", async () => {
  const selected = await open({ multiple: false, directory: false });
  if (selected) await openPath(selected);
});

document.getElementById("open-folder-btn").addEventListener("click", async () => {
  const selected = await open({ multiple: false, directory: true });
  if (selected) await openPath(selected);
});

// ── transport ──
playPauseBtn.addEventListener("click", async () => {
  if (!trackLoaded) {
    if (droneActive) {
      stopDrone();
      setPlayPauseLabel("play");
      nowPlayingEl.textContent = "no signal";
      setLiveIndicator(false);
    } else {
      startDrone();
      setPlayPauseLabel("pause");
      nowPlayingEl.textContent = "protocol drone";
      setLiveIndicator(true);
    }
    return;
  }

  const revision = ++playbackRevision;
  pendingToggles += 1;
  try {
    const status = await invoke("player_toggle");
    if (revision === playbackRevision) applyStatus(status);
  } catch (err) {
    console.error("play/pause failed:", err);
    nowPlayingEl.textContent = `playback error: ${err}`;
  } finally {
    pendingToggles -= 1;
  }
});

seekBar.addEventListener("mousedown", () => { isDraggingSeek = true; });
seekBar.addEventListener("change", async () => {
  const seconds = Number(seekBar.value);
  await invoke("player_seek", { seconds });
  isDraggingSeek = false;
});

volumeSlider.addEventListener("input", () => {
  pendingVolume = Number(volumeSlider.value);
  if (volumeTimer !== null) return;
  volumeTimer = setTimeout(async () => {
    volumeTimer = null;
    try {
      await invoke("player_set_volume", { volume: pendingVolume });
    } catch (err) {
      console.error("volume update failed:", err);
    }
  }, 50);
});

document.getElementById("next-btn").addEventListener("click", () => invoke("queue_next"));
document.getElementById("prev-btn").addEventListener("click", () => invoke("queue_prev"));
document.getElementById("close-btn").addEventListener("click", () => {
  saveState();
  window.__TAURI__.window.getCurrentWindow().close();
});

window.addEventListener("keydown", (e) => {
  if (e.target.tagName === "INPUT") return;
  if (e.code === "Space") {
    e.preventDefault();
    playPauseBtn.click();
  } else if (e.code === "ArrowRight") {
    document.getElementById("next-btn").click();
  } else if (e.code === "ArrowLeft") {
    document.getElementById("prev-btn").click();
  }
});

// ── status polling ──
function applyStatus(status) {
  lastKnownStatus = status;
  isPlaying = status.is_playing;
  targetBars = status.is_playing && Array.isArray(status.spectrum) && status.spectrum.length === BAR_COUNT
    ? status.spectrum
    : new Array(BAR_COUNT).fill(0);
  scheduleBars();
  volumeSlider.value = String(status.volume);
  setLiveIndicator(isPlaying || droneActive);

  if (status.queue_index !== null && status.queue_index !== undefined && libraryTracks[status.queue_index]) {
    const t = libraryTracks[status.queue_index];
    const titleText = `${t.title} — ${t.artist}`;
    nowPlayingEl.textContent = titleText;

    if (!mprisUnavailable && lastMprisTitle !== titleText) {
      lastMprisTitle = titleText;
      invoke("mpris_set_metadata", {
        title: t.title,
        artist: t.artist,
        durationSecs: status.duration_secs ?? null,
      }).catch((err) => {
        if (String(err).includes("MPRIS controls are unavailable")) {
          mprisUnavailable = true;
          console.warn("MPRIS metadata is unavailable; playback remains usable.");
        } else {
          console.error("MPRIS metadata update failed:", err);
        }
      });
    }

    trackListEl.querySelectorAll("li").forEach((li) => {
      li.classList.toggle("is-active", Number(li.dataset.index) === status.queue_index);
    });
  }

  setPlayPauseLabel(isPlaying ? "pause" : "play");

  const duration = status.duration_secs ?? 0;
  if (!isDraggingSeek) {
    seekBar.max = duration;
    seekBar.value = status.position_secs;
  }
  timeDisplay.textContent = `${formatTime(status.position_secs)} / ${formatTime(duration)}`;
}

async function pollStatus() {
  if (!trackLoaded || pendingToggles > 0 || statusPollInFlight) return;
  const now = performance.now();
  const interval = isPlaying ? 33 : 250;
  if (now - lastStatusPollAt < interval) return;
  lastStatusPollAt = now;
  statusPollInFlight = true;
  const revisionAtRequest = playbackRevision;
  try {
    const status = await invoke("player_status");
    if (revisionAtRequest === playbackRevision) applyStatus(status);
  } catch (err) {
    console.error("player status failed:", err);
  } finally {
    statusPollInFlight = false;
  }
}

setInterval(() => { void pollStatus(); }, 33);
setInterval(updateClock, 60_000);

updateClock();
resizeCanvas();
scheduleBars();
(async () => {
  const saved = await invoke("state_load");

  if (saved.volume != null) {
    volumeSlider.value = String(saved.volume);
    await invoke("player_set_volume", { volume: saved.volume });
  }

  if (saved.last_path) {
    lastOpenedPath = saved.last_path;
    try {
      libraryTracks = await invoke("library_scan_paths", { paths: [saved.last_path] });
      renderTrackList();
      if (libraryTracks.length > 0) {
        await playFromIndex(saved.last_index ?? 0, false);
        if (saved.last_position_secs) {
          await invoke("player_seek", { seconds: saved.last_position_secs });
        }
      }
    } catch (err) {
      console.error("failed to restore last session:", err);
    }
  }
})();
