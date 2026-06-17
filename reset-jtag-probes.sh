#!/usr/bin/env bash
#
# Reset (re-enumerate) ESP USB-Serial-JTAG probes on this host.
#
# Why: after a radio HIL test the support firmware keeps running radio in an
# infinite loop. Killing `probe-rs` only detaches the debugger, so that firmware
# keeps running and eventually wedges the *host-side* USB enumeration of the
# probe. Every subsequent `probe-rs`/`espflash` attach then fails with
# "Timeout during DMI access", and historically only `sudo reboot` healed it.
#
# Re-enumerating the device over USB is the targeted, no-reboot equivalent:
# `USBDEVFS_RESET` (_IO('U', 20)) only needs write access to the probe's
# /dev/bus/usb/<bus>/<dev> node -- the same access `probe-rs` already uses -- so
# this works as the (non-root) CI user.
#
# Only ESP USB-Serial-JTAG devices (VID:PID 303a:1001) are reset, so the RPI's
# other connected USB devices are left untouched.
#
# Usage:
#   reset-jtag-probes.sh            # reset every connected 303a:1001 probe
#   reset-jtag-probes.sh <serial>   # reset only the probe with this serial
#
# Env overrides (rarely needed):
#   JTAG_VID (default 303a), JTAG_PID (default 1001)

set -euo pipefail

VID="${JTAG_VID:-303a}"
PID="${JTAG_PID:-1001}"
WANT_SERIAL="${1:-}"

# _IO('U', 20)
USBDEVFS_RESET="0x5514"

if ! command -v python3 >/dev/null 2>&1; then
  echo "ERROR: python3 is required to issue the USBDEVFS_RESET ioctl" >&2
  exit 1
fi

reset_node() {
  # $1 = /dev/bus/usb/BBB/DDD, $2 = serial (for logging)
  local node="$1" serial="$2"
  echo "Resetting ESP-JTAG probe ${node} (serial: ${serial:-unknown})"
  python3 - "$node" "$USBDEVFS_RESET" <<'PY'
import fcntl, sys
node, req = sys.argv[1], int(sys.argv[2], 0)
with open(node, "wb", buffering=0) as f:
    fcntl.ioctl(f, req, 0)
PY
}

found=0
reset=0
for dev in /sys/bus/usb/devices/*; do
  # Only real devices expose idVendor/idProduct/busnum/devnum.
  [ -r "${dev}/idVendor" ]  || continue
  [ -r "${dev}/idProduct" ] || continue
  [ -r "${dev}/busnum" ]    || continue
  [ -r "${dev}/devnum" ]    || continue

  [ "$(cat "${dev}/idVendor")"  = "${VID}" ] || continue
  [ "$(cat "${dev}/idProduct")" = "${PID}" ] || continue

  serial="$(cat "${dev}/serial" 2>/dev/null || true)"
  if [ -n "${WANT_SERIAL}" ] && [ "${serial}" != "${WANT_SERIAL}" ]; then
    continue
  fi

  found=$((found + 1))
  busnum="$(cat "${dev}/busnum")"
  devnum="$(cat "${dev}/devnum")"
  node="$(printf '/dev/bus/usb/%03d/%03d' "${busnum}" "${devnum}")"

  if reset_node "${node}" "${serial}"; then
    reset=$((reset + 1))
  else
    echo "WARN: failed to reset ${node} (serial: ${serial:-unknown})" >&2
  fi
done

if [ "${found}" -eq 0 ]; then
  if [ -n "${WANT_SERIAL}" ]; then
    echo "No ESP-JTAG probe (${VID}:${PID}) found with serial '${WANT_SERIAL}'." >&2
  else
    echo "No ESP-JTAG probes (${VID}:${PID}) found." >&2
  fi
fi

echo "Reset ${reset}/${found} ESP-JTAG probe(s)."

# Give the kernel a moment to re-enumerate before anything attaches again.
sleep 2
