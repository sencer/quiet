#!/usr/bin/env bash
# Populates Quiet (or any FreeDesktop notification server) with sample test notifications:
# - 3 Gmail notifications (matches Chrome rule -> grouped under "Gmail")
# - 3 WhatsApp notifications (matches Chrome rule -> grouped under "WhatsApp")
# - 3 Ungrouped notifications (Spotify, Alacritty terminal, Power Manager critical)

set -euo pipefail

echo "==> Populating sample test notifications..."

# 1. Gmail notifications (Chrome with mail.google.com prefix)
notify-send -a "Google Chrome" -i "gmail" "Alice Smith" "mail.google.com\nHey, let's review the Q4 architecture plan."
notify-send -a "Google Chrome" -i "gmail" "Bob Jones" "mail.google.com\nTeam sync tomorrow at 10:00 AM"
notify-send -a "Google Chrome" -i "gmail" "GitHub Notifications" "mail.google.com\n[quiet] 3 new comments on PR #42"

# 2. WhatsApp notifications (Chrome with web.whatsapp.com prefix)
notify-send -a "Google Chrome" -i "whatsapp" "Mom" "web.whatsapp.com\nAre you coming over for dinner on Sunday?"
notify-send -a "Google Chrome" -i "whatsapp" "Mom" "web.whatsapp.com\nLet me know so I can cook your favorites!"
notify-send -a "Google Chrome" -i "whatsapp" "Dev Team" "web.whatsapp.com\nDeployment to production is complete 🚀"

# 3. Ungrouped notifications
notify-send -a "Spotify" -i "spotify" "Daft Punk" "Get Lucky (feat. Pharrell Williams)"
notify-send -a "Alacritty" -i "utilities-terminal" "Cargo Build" "Finished release [optimized] target(s) in 51.94s"
notify-send -a "Power Manager" -i "battery-caution" -u critical "Battery Low" "Battery level is at 18%, please plug in AC adapter."

echo "==> Done! Populated 9 notifications (3 Gmail, 3 WhatsApp, 3 ungrouped)."
echo "==> Run 'quietctl -t' or 'quiet -t' to view them in the UI dropdown."
