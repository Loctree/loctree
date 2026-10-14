#!/usr/bin/env bash
set -euo pipefail
workflow="${1:?expected publisher workflow path}"
scratch="$(mktemp -d)"
trap 'rm -rf "$scratch"' EXIT
mkdir -p "$scratch/bin" "$scratch/tmp"
grep -Fq 'ref: refs/tags/${{ inputs.tag }}' "$workflow"
python3 - "$workflow" "$scratch/verify.sh" <<'PY'
from pathlib import Path
import sys
lines=Path(sys.argv[1]).read_text().splitlines();start=next(i for i,l in enumerate(lines) if 'name: Verify signed release and download immutable bundles' in l);run=next(i for i in range(start,len(lines)) if lines[i].strip()=='run: |');body=[]
for line in lines[run+1:]:
 if line and not line.startswith('          '):break
 body.append(line[10:])
Path(sys.argv[2]).write_text('\n'.join(body)+'\n')
PY
cat > "$scratch/bin/gpg" <<'STUB'
#!/usr/bin/env bash
set -euo pipefail
if [[ "$*" == *--fingerprint* ]]; then
 printf 'pub:::::::::\nfpr:::::::::8868139E8A9A2291D067135FB979B60C7079E4D4:\nsub:::::::::\nfpr:::::::::1111111111111111111111111111111111111111:\n'
 if [[ "${EXTRA_PRIMARY:-0}" == 1 ]]; then
  printf 'pub:::::::::\nfpr:::::::::9999999999999999999999999999999999999999:\n'
 fi
fi
STUB
cat > "$scratch/bin/git" <<'STUB'
#!/usr/bin/env bash
set -euo pipefail
if [[ "$1" == rev-parse ]]; then
 if [[ "$2" == HEAD && "${HEAD_MISMATCH:-0}" == 1 ]]; then printf '%040d\n' 2; else printf '%040d\n' 1; fi
elif [[ "$1" != verify-tag ]]; then exit 2; fi
STUB
cat > "$scratch/bin/curl" <<'STUB'
#!/usr/bin/env bash
set -euo pipefail
while (( $# )); do
 if [[ "$1" == -o ]]; then : > "$2"; exit 0; fi
 shift
done
exit 2
STUB
cat > "$scratch/bin/gh" <<'STUB'
#!/usr/bin/env bash
set -euo pipefail
printf 'download\n' >> "$DOWNLOAD_LOG"
while (( $# )); do
 if [[ "$1" == --dir ]]; then
  touch "$2/fixture.tar.gz" "$2/fixture.tar.gz.sig" "$2/SHA256SUMS" "$2/SHA256SUMS.sig"
  exit 0
 fi
 shift
done
exit 2
STUB
printf '#!/usr/bin/env bash\nexit 0\n' > "$scratch/bin/sha256sum"
chmod +x "$scratch/bin/"*
export PATH="$scratch/bin:$PATH" RUNNER_TEMP="$scratch/tmp" GITHUB_ENV="$scratch/env" DOWNLOAD_LOG="$scratch/downloads" RELEASE_TAG=v0.15.0
: > "$DOWNLOAD_LOG"
EXTRA_PRIMARY=0 HEAD_MISMATCH=0 bash "$scratch/verify.sh"
[[ -s "$DOWNLOAD_LOG" ]]
: > "$DOWNLOAD_LOG"
if EXTRA_PRIMARY=1 HEAD_MISMATCH=0 bash "$scratch/verify.sh"; then echo 'untrusted extra primary key accepted' >&2; exit 1; fi
[[ ! -s "$DOWNLOAD_LOG" ]]
if EXTRA_PRIMARY=0 HEAD_MISMATCH=1 bash "$scratch/verify.sh"; then echo 'checkout differing from signed tag accepted' >&2; exit 1; fi
[[ ! -s "$DOWNLOAD_LOG" ]]
printf '%s\n' 'npm publisher identity contract: PASS'
