#!/usr/bin/env bash
# What ARIA's screen-time tracker can see on this session.
#
# The tracker picks its backend from the environment (see
# collectors::screentime::backend): a Wayland session goes through a compositor
# protocol, an X11 session through _NET_ACTIVE_WINDOW + MIT-SCREEN-SAVER. This
# prints the same inputs so you can tell "the tracker is broken" apart from
# "this session genuinely exposes nothing".
#
#   bash scripts/dev/screentime-probe.sh
set -u

say() { printf '\n\033[1m%s\033[0m\n' "$1"; }

say "session"
printf '  XDG_SESSION_TYPE = %s\n' "${XDG_SESSION_TYPE:-<unset>}"
printf '  XDG_CURRENT_DESKTOP = %s\n' "${XDG_CURRENT_DESKTOP:-<unset>}"
printf '  WAYLAND_DISPLAY = %s\n' "${WAYLAND_DISPLAY:-<unset>}"
printf '  DISPLAY = %s\n' "${DISPLAY:-<unset>}"

if [ -n "${WAYLAND_DISPLAY:-}" ]; then
  echo "  -> ARIA will use the Wayland backend"
elif [ -n "${DISPLAY:-}" ]; then
  echo "  -> ARIA will use the X11 backend"
else
  echo "  -> ARIA will report 'unsupported' and track nothing"
fi

if [ -n "${DISPLAY:-}" ]; then
  say "X11 focused window (_NET_ACTIVE_WINDOW -> WM_CLASS)"
  if ! command -v xprop >/dev/null; then
    echo "  xprop not installed (apt install x11-utils) — skipping"
  else
    win=$(xprop -root _NET_ACTIVE_WINDOW 2>/dev/null | grep -o '0x[0-9a-f]*' | head -1)
    if [ -z "$win" ]; then
      echo "  no _NET_ACTIVE_WINDOW — this WM doesn't publish EWMH focus"
    else
      printf '  window = %s\n' "$win"
      xprop -id "$win" WM_CLASS _NET_WM_NAME 2>/dev/null | sed 's/^/  /'
    fi
  fi

  say "X11 idle (MIT-SCREEN-SAVER)"
  if command -v xprintidle >/dev/null; then
    printf '  %s ms since last input\n' "$(xprintidle)"
  else
    echo "  xprintidle not installed (apt install xprintidle) — ARIA reads the"
    echo "  same extension directly via x11rb, so this is only a cross-check."
  fi
fi

if [ -n "${WAYLAND_DISPLAY:-}" ]; then
  say "Wayland globals ARIA looks for"
  if command -v wayland-info >/dev/null; then
    wayland-info 2>/dev/null | grep -E \
      'zcosmic_toplevel_info_v1|zwlr_foreign_toplevel_manager_v1|ext_idle_notifier_v1' \
      | sed 's/^/  /' || echo "  none of them advertised"
    echo
    echo "  cosmic/wlr toplevel manager -> per-app breakdown"
    echo "  ext_idle_notifier only      -> total screen time, no breakdown"
  else
    echo "  wayland-info not installed (apt install wayland-utils) — can't enumerate."
    echo "  GNOME/Mutter is known to offer ext_idle_notifier_v1 but no toplevel"
    echo "  protocol, so it reports total time with no per-app rows."
  fi
fi

say "ARIA's own tally"
data="${XDG_DATA_HOME:-$HOME/.local/share}"
for dir in "$data"/com.aria.* "$data"/aria; do
  [ -d "$dir/screentime" ] || continue
  printf '  %s\n' "$dir/screentime"
  ls -1t "$dir/screentime" | head -3 | sed 's/^/    /'
done
echo
