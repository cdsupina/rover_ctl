# rover_ctl

Terminal-based UDP teleop client for a 4-wheel omni-drive rover. Reads keyboard
events, streams body-frame velocity commands over UDP at 10 Hz.

Written for a specific Raspberry Pi Pico 2W rover firmware but the wire protocol
is trivial (ASCII triplet `<vx> <vy> <omega>` on UDP) — easy to point at
anything else that listens for the same format.

## Install / run

```sh
git clone https://github.com/<you>/rover_ctl.git
cd rover_ctl
cargo run --release                          # default target: 192.168.1.102:4242
cargo run --release -- 192.168.1.55          # custom host
cargo run --release -- 192.168.1.55:4242     # custom host + port
```

## Controls

| Key            | Action                                        |
|----------------|-----------------------------------------------|
| **W / S**      | forward / backward                            |
| **A / D**      | strafe left / right                           |
| **Q / E**      | rotate CCW / CW                               |
| **Space** (or X) | stop                                        |
| **+ / −**      | speed up / down (100–1000 permille, step 100) |
| **Esc / Ctrl-C** | quit (sends a final `stop`)                 |

Combine keys for diagonal / curving motion. Adjusting speed while a key is held
takes effect immediately — no need to release and re-press.

## Two input modes (auto-detected)

- **hold-to-drive** — when the terminal supports the
  [kitty keyboard protocol](https://sw.kovidgoyal.net/kitty/keyboard-protocol/)
  (kitty, ghostty, foot, wezterm with `enable_kitty_keyboard = true`,
  alacritty ≥ 0.13). Pressing a key sets velocity, releasing zeros it.
- **sticky** — fallback for terminals that don't report key release events
  (xterm, gnome-terminal, etc.). Keys SET the velocity; press Space to stop.

The current mode is shown at the top of the UI.

## Wire protocol

Each UDP datagram is one ASCII line:

```
<vx> <vy> <omega>
```

three space-separated integers in permille (`-1000..1000`), body-frame:

- `vx`: strafe — positive = right
- `vy`: forward — positive = forward
- `omega`: yaw — positive = CCW

`stop` or `s` is accepted as a synonym for `0 0 0`.

Client streams the current state at 10 Hz so the rover's watchdog stays fed.
On quit it sends a single `stop` and exits.

## License

MIT — see [LICENSE](LICENSE).
