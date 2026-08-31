#!/usr/bin/env python3
"""A fake MPRIS2 player, for exercising ARIA's NOW PLAYING widget on demand.

Real players make the interesting cases hard to reach: you cannot ask Spotify
for a 200-character title, and you cannot ask it to stop reporting a position.
This publishes a normal MPRIS2 service that `playerctl` — and therefore ARIA —
treats like any other player, with every awkward property under a flag.

Needs python3-dbus and python3-gi (both ship with Pop!_OS/GNOME):

    sudo apt install python3-dbus python3-gi

Examples:

    # A long title, to check the cover art survives it.
    ./scripts/dev/fake-mpris.py --title-len 200

    # A player that advertises a position but never advances it.
    ./scripts/dev/fake-mpris.py --frozen-position

    # A player with no position at all (the property errors).
    ./scripts/dev/fake-mpris.py --no-position

    # A cover URL that 404s — the widget must not show a broken image.
    ./scripts/dev/fake-mpris.py --art 404

    # A local file:// cover, the shape Chromium-based browsers publish.
    ./scripts/dev/fake-mpris.py --art file

Verify what ARIA will see:

    playerctl -p ariafake metadata
    playerctl -p ariafake position
"""

import argparse
import base64
import os
import struct
import sys
import tempfile
import time
import zlib

try:
    import dbus
    import dbus.mainloop.glib
    import dbus.service
    from gi.repository import GLib
except ImportError as e:  # pragma: no cover - dependency hint only
    sys.exit(f"{e}\n\nInstall the deps:  sudo apt install python3-dbus python3-gi")

BUS_NAME = "org.mpris.MediaPlayer2.ariafake"
OBJ_PATH = "/org/mpris/MediaPlayer2"
ROOT_IFACE = "org.mpris.MediaPlayer2"
PLAYER_IFACE = "org.mpris.MediaPlayer2.Player"
PROPS_IFACE = "org.freedesktop.DBus.Properties"

def tiny_png(size=64, rgb=(214, 132, 42)):
    """A valid solid-colour PNG, built here so --art file/data needs no image
    library, no network and no particular icon to be installed.

    Written by hand rather than pasted as a base64 blob: a blob with a wrong
    CRC still opens in lenient viewers but is rejected by WebKit, which makes
    for a confusing test fixture (it looks like ARIA dropped the cover).
    """
    raw = b"".join(b"\x00" + bytes(rgb) * size for _ in range(size))  # filter 0 per row

    def chunk(kind, data):
        return (
            struct.pack(">I", len(data))
            + kind
            + data
            + struct.pack(">I", zlib.crc32(kind + data) & 0xFFFFFFFF)
        )

    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", struct.pack(">IIBBBBB", size, size, 8, 2, 0, 0, 0))  # 8-bit RGB
        + chunk(b"IDAT", zlib.compress(raw, 9))
        + chunk(b"IEND", b"")
    )


ART_HTTP = "https://placehold.co/300x300.png"
# Resolves, then 404s — the widget must not render a broken image for it.
ART_404 = "https://placehold.co/this-path-does-not-exist-4f2a1b.png"


def art_url(choice):
    """The mpris:artUrl to publish, covering every shape ARIA has to handle."""
    if choice == "none":
        return ""
    if choice == "http":
        return ART_HTTP
    if choice == "404":
        return ART_404
    if choice == "data":
        return "data:image/png;base64," + base64.b64encode(tiny_png()).decode()
    # "file": Chromium-style local cover. Written on demand so the script has
    # no dependency on any particular icon being installed.
    path = os.path.join(tempfile.gettempdir(), "aria-fake-cover.png")
    with open(path, "wb") as fh:
        fh.write(tiny_png())
    return "file://" + path


ART_CHOICES = ["none", "http", "404", "data", "file"]


class FakePlayer(dbus.service.Object):
    def __init__(self, bus, args):
        super().__init__(dbus.service.BusName(BUS_NAME, bus), OBJ_PATH)
        self.args = args
        self.started = time.monotonic()
        self.paused_at = None if args.playing else 0.0

    # ---- the position the player claims to be at -------------------------
    def position_us(self):
        if self.args.frozen_position:
            return 0  # advertises a position, never advances it
        if self.paused_at is not None:
            return int(self.paused_at * 1e6)
        elapsed = time.monotonic() - self.started
        if self.args.length > 0:
            elapsed %= self.args.length  # loop rather than run off the end
        return int(elapsed * 1e6)

    def metadata(self):
        title = self.args.title or "Fake Track"
        if self.args.title_len:
            # Word-ish filler so the ellipsis has somewhere to land.
            title = ("Interminably Long Fake Track Title " * 40)[: self.args.title_len].strip()
        m = {
            "mpris:trackid": dbus.ObjectPath("/org/aria/fake/track/1"),
            "xesam:title": dbus.String(title),
            "xesam:artist": dbus.Array([dbus.String(self.args.artist)], signature="s"),
            "xesam:album": dbus.String(self.args.album),
        }
        if self.args.length > 0:
            m["mpris:length"] = dbus.Int64(int(self.args.length * 1e6))
        art = art_url(self.args.art)
        if art:
            m["mpris:artUrl"] = dbus.String(art)
        return dbus.Dictionary(m, signature="sv")

    def player_props(self):
        return {
            "PlaybackStatus": dbus.String("Paused" if self.paused_at is not None else "Playing"),
            "Metadata": self.metadata(),
            "Position": dbus.Int64(self.position_us()),
            "Rate": dbus.Double(1.0),
            "MinimumRate": dbus.Double(1.0),
            "MaximumRate": dbus.Double(1.0),
            "Volume": dbus.Double(1.0),
            "CanGoNext": dbus.Boolean(True),
            "CanGoPrevious": dbus.Boolean(True),
            "CanPlay": dbus.Boolean(True),
            "CanPause": dbus.Boolean(True),
            "CanSeek": dbus.Boolean(not self.args.no_position),
            "CanControl": dbus.Boolean(True),
        }

    # ---- org.freedesktop.DBus.Properties ---------------------------------
    @dbus.service.method(PROPS_IFACE, in_signature="ss", out_signature="v")
    def Get(self, iface, prop):
        if iface == PLAYER_IFACE and prop == "Position" and self.args.no_position:
            raise dbus.exceptions.DBusException(
                "org.freedesktop.DBus.Error.NotSupported", "Position is not supported"
            )
        props = self.GetAll(iface)
        if prop not in props:
            raise dbus.exceptions.DBusException(
                "org.freedesktop.DBus.Error.InvalidArgs", f"No such property {prop}"
            )
        return props[prop]

    @dbus.service.method(PROPS_IFACE, in_signature="s", out_signature="a{sv}")
    def GetAll(self, iface):
        if iface == PLAYER_IFACE:
            props = self.player_props()
            if self.args.no_position:
                props.pop("Position")
            return dbus.Dictionary(props, signature="sv")
        if iface == ROOT_IFACE:
            return dbus.Dictionary(
                {
                    "CanQuit": dbus.Boolean(True),
                    "CanRaise": dbus.Boolean(False),
                    "HasTrackList": dbus.Boolean(False),
                    "Identity": dbus.String("ARIA fake player"),
                    "DesktopEntry": dbus.String("ariafake"),
                    "SupportedUriSchemes": dbus.Array([], signature="s"),
                    "SupportedMimeTypes": dbus.Array([], signature="s"),
                },
                signature="sv",
            )
        return dbus.Dictionary({}, signature="sv")

    @dbus.service.method(PROPS_IFACE, in_signature="ssv")
    def Set(self, iface, prop, value):
        pass

    @dbus.service.signal(PROPS_IFACE, signature="sa{sv}as")
    def PropertiesChanged(self, iface, changed, invalidated):
        pass

    # ---- transport, so ARIA's buttons do something visible ---------------
    def _announce(self):
        props = self.player_props()
        self.PropertiesChanged(
            PLAYER_IFACE,
            dbus.Dictionary(
                {"PlaybackStatus": props["PlaybackStatus"], "Metadata": props["Metadata"]},
                signature="sv",
            ),
            [],
        )

    @dbus.service.method(PLAYER_IFACE)
    def PlayPause(self):
        if self.paused_at is None:
            self.paused_at = self.position_us() / 1e6
        else:
            self.started = time.monotonic() - self.paused_at
            self.paused_at = None
        print(f"PlayPause -> {'Paused' if self.paused_at is not None else 'Playing'}", flush=True)
        self._announce()

    @dbus.service.method(PLAYER_IFACE)
    def Next(self):
        self.started = time.monotonic()
        print("Next", flush=True)
        self._announce()

    @dbus.service.method(PLAYER_IFACE)
    def Previous(self):
        self.started = time.monotonic()
        print("Previous", flush=True)
        self._announce()

    @dbus.service.method(PLAYER_IFACE, in_signature="x")
    def Seek(self, offset_us):
        self.started -= offset_us / 1e6
        self._announce()


def main():
    p = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    p.add_argument("--title", default="Fake Track")
    p.add_argument("--artist", default="ARIA Test Suite")
    p.add_argument("--album", default="Diagnostics")
    p.add_argument(
        "--title-len",
        type=int,
        default=0,
        metavar="N",
        help="replace the title with N characters of filler (try 200)",
    )
    p.add_argument(
        "--length",
        type=float,
        default=215.0,
        metavar="SECS",
        help="track length; 0 publishes no mpris:length at all",
    )
    p.add_argument(
        "--art",
        choices=ART_CHOICES,
        default="http",
        help="cover shape to publish: a remote URL, one that 404s, an inlined "
        "data: URI, a local file://, or none at all",
    )
    p.add_argument(
        "--no-position",
        action="store_true",
        help="the Position property errors, like a player that has none",
    )
    p.add_argument(
        "--frozen-position",
        action="store_true",
        help="Position always reads 0, like a stuck media session",
    )
    p.add_argument("--paused", dest="playing", action="store_false", help="start paused")
    args = p.parse_args()

    dbus.mainloop.glib.DBusGMainLoop(set_as_default=True)
    FakePlayer(dbus.SessionBus(), args)
    print(f"fake MPRIS player up as {BUS_NAME} — Ctrl-C to stop", flush=True)
    print("  playerctl -p ariafake metadata", flush=True)
    try:
        GLib.MainLoop().run()
    except KeyboardInterrupt:
        print()


if __name__ == "__main__":
    main()
