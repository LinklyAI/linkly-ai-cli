#!/bin/sh
# test-install.sh - Run scripts/install.sh against a local file:// release
# fixture and check that it installs good releases and refuses bad ones.
# Usage: scripts/test-install.sh [shell ...]   (default: every one of sh, dash, bash found)
# Nothing touches the real HOME: each case gets its own temporary HOME.
set -eu

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
INSTALLER="${SCRIPT_DIR}/install.sh"
VERSION="9.9.9"
TARGETS="aarch64-apple-darwin x86_64-apple-darwin x86_64-unknown-linux-gnu aarch64-unknown-linux-gnu"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

sha256_of() {
    if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | awk '{ print $1 }'
    else shasum -a 256 "$1" | awk '{ print $1 }'; fi
}

# Fixture: <root>/latest.json, <root>/v<version>/{SHA256SUMS,linkly-<target>.tar.gz}
# shaped like what release.yml publishes.
make_release() {
    root="$1"
    mkdir -p "$root/v$VERSION" "$work/bin"
    printf '#!/bin/sh\necho "linkly %s"\n' "$VERSION" > "$work/bin/linkly"
    chmod +x "$work/bin/linkly"
    : > "$root/v$VERSION/SHA256SUMS"
    for t in $TARGETS; do
        tar czf "$root/v$VERSION/linkly-$t.tar.gz" -C "$work/bin" linkly
        printf '%s  %s\n' "$(sha256_of "$root/v$VERSION/linkly-$t.tar.gz")" "linkly-$t.tar.gz" \
            >> "$root/v$VERSION/SHA256SUMS"
    done
    # The live manifest shape, including the sha256 map that broke the old parser.
    cat > "$root/latest.json" <<JSON
{
  "version": "$VERSION",
  "assets": {
    "darwin-aarch64": "https://example.invalid/linkly-aarch64-apple-darwin.tar.gz",
    "linux-x86_64": "https://example.invalid/linkly-x86_64-unknown-linux-gnu.tar.gz"
  },
  "sha256": {
    "darwin-aarch64": "0000000000000000000000000000000000000000000000000000000000000000",
    "linux-x86_64": "0000000000000000000000000000000000000000000000000000000000000000"
  }
}
JSON
}

failures=0
pass() { printf '  ok    %s\n' "$1"; }
fail() { printf '  FAIL  %s\n' "$1"; failures=$((failures + 1)); }

# run_case <shell> <name> <expect: ok|fail> <stderr substring or ""> <mutator>
# The mutator edits a fresh copy of the fixture to set up the case.
run_case() {
    sh_bin="$1"; name="$2"; expect="$3"; want_msg="$4"; mutate="$5"
    root="$work/case/root"; home="$work/case/home"
    rm -rf "$work/case"; mkdir -p "$home"
    make_release "$root"
    "$mutate" "$root"

    set +e
    out="$(HOME="$home" PATH="$home/.linkly/bin:$PATH" LINKLY_DOWNLOAD_BASE="file://$root" \
        "$sh_bin" "$INSTALLER" 2>&1)"
    status=$?
    set -e

    installed="$home/.linkly/bin/linkly"
    if [ "$expect" = ok ]; then
        if [ "$status" -eq 0 ] && [ "$("$installed")" = "linkly $VERSION" ]; then
            pass "$name"
        else
            fail "$name (exit $status)"; printf '%s\n' "$out" | sed 's/^/        /'
        fi
    else
        if [ "$status" -ne 0 ] && [ ! -e "$installed" ] && printf '%s' "$out" | grep -qF "$want_msg"; then
            pass "$name"
        else
            fail "$name (exit $status, installed: $([ -e "$installed" ] && echo yes || echo no))"
            printf '%s\n' "$out" | sed 's/^/        /'
        fi
    fi
}

as_published() { :; }
minified_crlf() {
    printf '{"version":"%s","sha256":{"linux-x86_64":"x"},"assets":{"linux-x86_64":"y"}}\r\n' "$VERSION" > "$1/latest.json"
}
extra_version_key() {
    printf '{"version":"%s","channels":{"beta":{"version":"10.0.0-beta.1"}}}\n' "$VERSION" > "$1/latest.json"
}
bad_version() { printf '{"version":"latest"}\n' > "$1/latest.json"; }
tampered_archive() {
    for t in $TARGETS; do printf 'garbage' >> "$1/v$VERSION/linkly-$t.tar.gz"; done
}
platform_missing() { : > "$1/v$VERSION/SHA256SUMS"; }
missing_release_dir() { printf '{"version":"9.9.8"}\n' > "$1/latest.json"; }

shells="${*:-}"
if [ -z "$shells" ]; then
    for s in sh dash bash; do command -v "$s" >/dev/null 2>&1 && shells="$shells $s"; done
fi

for s in $shells; do
    echo "== $s"
    run_case "$s" "installs from the published manifest shape" ok "" as_published
    run_case "$s" "accepts a minified CRLF manifest" ok "" minified_crlf
    run_case "$s" "refuses an ambiguous manifest" fail "Unexpected latest.json format" extra_version_key
    run_case "$s" "refuses a non-semver version" fail "Invalid version" bad_version
    run_case "$s" "refuses a tampered archive" fail "Checksum mismatch" tampered_archive
    run_case "$s" "refuses a platform missing from SHA256SUMS" fail "No valid checksum" platform_missing
    run_case "$s" "fails when the release files are absent" fail "Download failed" missing_release_dir
done

if [ "$failures" -ne 0 ]; then
    echo "$failures case(s) failed"
    exit 1
fi
echo "all cases passed"
