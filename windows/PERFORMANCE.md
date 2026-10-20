# Gaming performance

Enable **General > Game optimization** to use automatic gaming suspension on Windows.
This setting defaults to off and works independently of the optional visual Game pill.
When a game is open and optimization is off, the pill offers **Optimalisation**
to navigate to the switch without changing it automatically.
When a supported game is in the foreground, Coucou parks the island and desktop
Mochi off-screen, parks both cursor workers, pauses JavaScript and CSS animations,
stops beat capture/analysis and character sounds, and defers optional integration
and Spotify refreshes. Foreground and cursor-visibility changes are observed through Windows events.
The three-second local check remains as a fallback. Hidden system cursors never
participate in overlay hit-testing, including games not recognized by name.
Native click-through and capturing DOM guards both block hover/click delivery.

By default, alt-tab restores the overlays and animation clocks.
With **Automatically turn on when a game is open** enabled, optimization instead
stays active while the detected game process is running, including in the background.
This follows the same retained game state as the visual Game pill. General remains
interactive so either switch can be changed while optimization is active. The Game card retains the
game name until the tracked process closes. Disabling Game optimization also
restores normal operation; hiding the Game pill does not disable optimization. Agent approval handling remains available in the
terminal while the overlays are parked.

Outside games, island animation is capped at 60 fps and decorative hub animation
at 30 fps. The extra 80 ms island watchdog was removed: the cancellable frame
clock already provides a 32 ms fallback for withheld WebView2 animation frames.
WebView2 background throttling and native occlusion are enabled again, with
matching browser arguments for every window.

Validation includes cancellation of pending and late animation callbacks during
gameplay, resumption after alt-tab, and parking/resuming native cursor workers
without waking a collapsed island. Actual FPS and frame-time improvement needs
an A/B comparison in the same game scene with the new executable; these changes
do not establish a measured FPS gain or cover games the detector cannot identify.


The Game card shows the game process session time (including background time),
plus requests reported as waiting by connected coding agents. The counter is
separate from the single approval card, deduplicates repeated hook events, and
clears requests on decisions or the matching terminal/session progress. It works
for built-in and custom agent tags without adding approval permissions. The
session clock only wakes at the next displayed minute while the card is visible,
and stops while hidden or gaming suspension is active.
