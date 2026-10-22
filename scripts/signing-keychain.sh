#!/bin/zsh
# Makes a keychain that holds only the app's signing identity, unlocked, and usable by
# codesign without asking: never your login keychain, so signing can't prompt you.
# Usage: scripts/signing-keychain.sh <signing.p12> <p12 password> <keychain path> <keychain password>
# Prints the identity's SHA-1, which codesign takes as --sign.
set -euo pipefail

P12="${1:?usage: scripts/signing-keychain.sh <signing.p12> <p12 password> <keychain> <keychain password>}"
P12_PASSWORD="$2"
KEYCHAIN="$3"
KEYCHAIN_PASSWORD="$4"

# Creating a keychain can add it to the search list; put the list back as it was.
SEARCH_LIST=("${(@f)$(security list-keychains -d user | sed -e 's/^ *"//' -e 's/"$//')}")
security delete-keychain "$KEYCHAIN" 2>/dev/null || true
security create-keychain -p "$KEYCHAIN_PASSWORD" "$KEYCHAIN"
security list-keychains -d user -s "${SEARCH_LIST[@]}"
security set-keychain-settings "$KEYCHAIN"   # no locking after a timeout
security unlock-keychain -p "$KEYCHAIN_PASSWORD" "$KEYCHAIN"
security import "$P12" -k "$KEYCHAIN" -P "$P12_PASSWORD" -f pkcs12 -T /usr/bin/codesign >/dev/null
security set-key-partition-list -S apple-tool:,apple:,codesign: -s -k "$KEYCHAIN_PASSWORD" "$KEYCHAIN" >/dev/null
security find-certificate -a -Z "$KEYCHAIN" | awk '/SHA-1 hash:/ { print $3; exit }'
