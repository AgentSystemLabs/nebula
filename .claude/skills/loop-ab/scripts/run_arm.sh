#!/bin/bash
# usage: AB=<ab-dir> run_arm.sh <arm-dir> <label>
# One headless run of $AB/feature-prompt.md inside one arm's clone, under the same classifier-backed
# "auto" permission mode interactive sessions use. Detach it: `nohup run_arm.sh … > /dev/null 2>&1 &`
# (a background tool task does not survive a session restart; the markers below do).
set -u
: "${AB:?set AB to the A/B directory}"
dir="$1"; label="$2"; out="$AB/out"; mkdir -p "$out"
cd "$dir" || exit 2
date -u +%FT%TZ > "$out/$label.started"
start=$(date +%s)
env -u NEBULA_AGENT_ID -u NEBULA_API_URL -u NEBULA_API_TOKEN -u CLAUDECODE -u CLAUDE_CODE_ENTRYPOINT \
  claude -p "$(cat "$AB/feature-prompt.md")" \
    --model "${AB_MODEL:-claude-fable-5-1}" \
    --output-format stream-json --verbose \
    --permission-mode auto --permission-prompts none \
    > "$out/$label.stream.jsonl" 2> "$out/$label.stderr.log"
echo $? > "$out/$label.exit"
echo $(( $(date +%s) - start )) > "$out/$label.wall_seconds"
date -u +%FT%TZ > "$out/$label.finished"
