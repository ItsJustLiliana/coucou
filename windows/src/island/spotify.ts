// Spotify's reports → the page (src-tauri/src/spotify.rs). The parts of
// SpotifyController.swift that touch the app: the pill wears the track's
// title, music starting shows the compact island without a sound, and the
// card reads the player again whenever it comes on screen (seeks made in
// Spotify's own window are never signalled).

import { Bridge, onEvent } from "../core/bridge";
import { pillDefinition } from "../core/pills";
import { SPOTIFY_ID, Spotify, isAd, spotifyPosition, reconcileSpotify, type SpotifyState } from "../core/spotify";
import { State } from "../core/state";
import { reacquireBeat } from "../mochi/beat";

import { t } from "../i18n/i18n";

export interface SpotifyHost {
  /** Compact island from hidden, no peek sound. */
  revealSilently(): void;
}

export function registerSpotifyHandlers(island: SpotifyHost) {
  void onEvent<SpotifyState>("spotify", (s) => applySpotify(island, s));
  void onEvent<{ artUrl: string; dataUrl: string }>("spotify-artwork", (art) => {
    Spotify.artwork = art;
    State.notify();
  });
  // Check song timeline rather than elapsed capture time: starting halfway
  // through a track must not schedule a false 30-second check.
  window.setInterval(checkTempoMilestones, 500);
  State.subscribe(() => {
    syncPillName();
    refreshWhenShown();
  });
}

/** A report from Rust (or the answer to a refresh). */
export function applySpotify(island: SpotifyHost, next: SpotifyState) {
  const wasPlaying = State.spotifyPlaying;
  const previous = Spotify.state;
  next = reconcileSpotify(previous, next);
  if (previous.track?.id && previous.track.id === next.track?.id && previous.playing && next.playing &&
      Math.abs(next.position - spotifyPosition(previous, next.positionAt)) > 1.25) {
    Spotify.beatRevision++;
  }
  Spotify.state = next;
  syncPillName();
  // Only on not playing → playing (SpotifyController.setPlaying).
  if (!wasPlaying && State.spotifyPlaying && !State.paused && State.mode === "hidden") {
    island.revealSilently();
  }
  State.notify();
}

/** SpotifyController.syncTaskName: the track's title, else the pill's name. */
export function syncPillName() {
  const task = State.tasks.find((x) => x.id === SPOTIFY_ID);
  if (!task) return;
  const track = Spotify.state.track;
  const title = isAd(track) ? t("Advertisement") : (track?.title ?? "");
  const name = title || (pillDefinition(SPOTIFY_ID)?.name ?? "Spotify");
  if (task.name !== name) task.name = name;
}

let checkedTrack = "";
let previousPosition: number | null = null;
const checkedMilestones = new Set<number>();
function checkTempoMilestones() {
  const state = Spotify.state;
  const track = state.track;
  if (!track || track.id !== checkedTrack) {
    checkedTrack = track?.id ?? "";
    checkedMilestones.clear();
    previousPosition = track ? spotifyPosition(state, Date.now()) : null;
    return;
  }
  const now = spotifyPosition(state, Date.now());
  if (state.playing && State.settings.beatSync && previousPosition !== null &&
      now >= previousPosition) {
    for (const mark of [30, 60]) {
      if (!checkedMilestones.has(mark) && previousPosition < mark && now >= mark &&
          now - previousPosition <= 10) {
        checkedMilestones.add(mark);
        // Require six fresh consistent reports before adopting a new tempo.
        // The current dance BPM stays intact during this review.
        reacquireBeat();
        void Bridge.beatReview();
      }
    }
  }
  previousPosition = now;
}

let shown = false;

function refreshWhenShown() {
  const now = State.mode === "expanded" && State.view === "overview" && State.focusTask?.id === SPOTIFY_ID;
  if (now && !shown) void Bridge.spotifyRefresh();
  shown = now;
}
