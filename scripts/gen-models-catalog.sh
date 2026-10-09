#!/usr/bin/env bash
#
# Regenerates data/models.json — the static model catalog embedded into the
# binary (see src/models_catalog.rs) — and data/opencode-transports.json —
# the per-model transport map for the OpenCode Zen/Go gateways (see
# OpencodeTransport in src/provider.rs). Source: https://models.dev/api.json,
# which lists every provider's model ids.
#
# The committed JSON files are the single build-time source of truth, so the
# build stays offline and reproducible. Run this (or let the scheduled CI
# workflow .github/workflows/update-models.yml run it) to refresh the snapshot.
#
# Usage: scripts/gen-models-catalog.sh
# Requires: curl, jq
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
OUT="$SCRIPT_DIR/../data/models.json"
OUT_TRANSPORTS="$SCRIPT_DIR/../data/opencode-transports.json"
SRC="https://models.dev/api.json"

# Vendors kept for the curated OpenRouter subset (its full list is ~340 models;
# the rest are reachable on demand via `/models refresh`).
OPENROUTER_VENDORS='["anthropic","openai","google","deepseek","x-ai","meta-llama","mistralai","qwen","moonshotai","z-ai"]'

# models.dev keeps the FULL history of each provider, including retired model ids
# the provider's API now rejects. We can't read "retired" from the data, so we
# drop ids whose last_updated/release_date is older than a per-vendor cutoff.
#
# Cutoffs below were derived by cross-checking models.dev against each provider's
# LIVE /models API on 2026-06-04:
#   anthropic  — retires aggressively; models.dev still lists the 2024 claude-3
#                line (incl. claude-3-7-sonnet, 2025-02) that the API rejects.
#                2025-04-01 drops those and keeps the claude-4.x line + aliases.
#   openai/gemini/openrouter — ZERO stale ids found (these providers keep listed
#                models valid, and openrouter prunes its own), so NO cutoff: a
#                cutoff would only delete still-valid models (e.g. gemini-2.0-flash,
#                gpt-4o). Keys are by zerostack provider name; "0000-00-00" = keep all.
# This is a heuristic snapshot — the authoritative list is always the provider's
# live API, reachable via `/models refresh`. Re-tune when a vendor retires a line.
CUTOFFS='{"anthropic":"2025-04-01"}'

echo "Fetching $SRC ... (cutoffs: $CUTOFFS)" >&2
api="$(curl -fsSL --max-time 120 "$SRC")"

# jq program:
#  - `entry`  : project a models.dev model into our compact {id,name,context}.
#  - `priced_entry` : `entry` plus input_price/output_price (USD per million
#               tokens) straight from models.dev's `cost.input`/`cost.output`,
#               when present. Used for the direct-API providers (anthropic,
#               openai, gemini), which have no other source of pricing at
#               runtime. OpenRouter keeps plain `entry`: it has its own live
#               pricing fetch (`fetch_openrouter_pricing`), and models.dev's
#               marketplace-wide OpenRouter cost data is less reliable.
#  - `denied` : drop non-chat models (embeddings/audio/image/etc.) by id substring,
#               mirroring crate::provider::is_agent_model's denylist, and keep only
#               models that can output text.
#  - emits keys by *zerostack* provider name (gemini <- models.dev "google").
echo "$api" | jq --argjson orv "$OPENROUTER_VENDORS" --argjson cut "$CUTOFFS" '
  def deny: [
    "embedding","embed-","text-embedding","gemini-embedding","whisper","transcribe",
    "tts","-audio","realtime","speech","dall-e","gpt-image","image-generation",
    "imagen","sora","veo","moderation","rerank","aqa","davinci-002","babbage-002",
    "stable-diffusion","flux"
  ];
  # ISO date strings compare correctly lexicographically; $c is the vendor cutoff.
  def recent($c): select((.value.last_updated // .value.release_date // "0000") >= $c);
  def chat($c):
    select(.value.modalities.output | index("text"))
    | select((.key | ascii_downcase) as $id | (deny | any(. as $d | $id | contains($d))) | not)
    | recent($c);
  def entry: {id: .value.id, name: .value.name, context: (.value.limit.context // null)};
  def priced_entry: entry + {
    input_price: (.value.cost.input // null),
    output_price: (.value.cost.output // null)
  };
  def models_of($p; $c; e): ($p.models // {}) | to_entries | map(chat($c) | e) | sort_by(.id);
  # OpenCode Zen/Go slices: priced entries like the direct-API providers (the
  # gateways' live /models listing carries no pricing). Retired ids are
  # dropped via `status` (OpenCode marks them, unlike the date-cutoff vendors
  # above), and `@ai-sdk/google` models are skipped — zerostack only speaks
  # the chat/responses/messages transports, not the Gemini-native endpoint.
  def opencode_models($p): ($p.models // {})
    | to_entries
    | map(select(.value.status != "deprecated")
          | select((.value.provider.npm // "") != "@ai-sdk/google")
          | chat("0000-00-00") | priced_entry)
    | sort_by(.id);
  {
    anthropic:  models_of(.anthropic; ($cut.anthropic  // "0000-00-00"); priced_entry),
    openai:     models_of(.openai;    ($cut.openai     // "0000-00-00"); priced_entry),
    gemini:     models_of(.google;    ($cut.gemini     // "0000-00-00"); priced_entry),
    openrouter: (
      (.openrouter.models // {})
      | to_entries
      | map(chat($cut.openrouter // "0000-00-00")
            | select((.key | split("/")[0]) as $v | $orv | index($v)) | entry)
      | sort_by(.id)
    ),
    "opencode-go":  opencode_models(."opencode-go"),
    "opencode-zen": opencode_models(.opencode)
  }
' > "$OUT"

# Per-model transport for the OpenCode gateways: one provider id serves three
# API shapes, and the live /models listing says nothing about which model
# needs which. models.dev records the AI SDK package per model, which maps
# 1:1 onto the transport (`@ai-sdk/openai` hits /responses, `@ai-sdk/anthropic`
# hits /messages, everything else hits /chat/completions). Same filters as the
# slices above; models absent here fall back to chat at runtime with a warning.
echo "$api" | jq '
  def deny: [
    "embedding","embed-","text-embedding","gemini-embedding","whisper","transcribe",
    "tts","-audio","realtime","speech","dall-e","gpt-image","image-generation",
    "imagen","sora","veo","moderation","rerank","aqa","davinci-002","babbage-002",
    "stable-diffusion","flux"
  ];
  def chat:
    select(.value.modalities.output | index("text"))
    | select((.key | ascii_downcase) as $id | (deny | any(. as $d | $id | contains($d))) | not);
  def transport: (.provider.npm // "") as $n
    | if $n == "@ai-sdk/openai" then "responses"
      elif $n == "@ai-sdk/anthropic" then "messages"
      else "chat" end;
  def opencode_transports($p): ($p.models // {})
    | to_entries
    | map(select(.value.status != "deprecated")
          | select((.value.provider.npm // "") != "@ai-sdk/google")
          | chat
          | {key: .key, value: (.value | transport)})
    | sort_by(.key)
    | from_entries;
  {
    "opencode-go":  opencode_transports(."opencode-go"),
    "opencode-zen": opencode_transports(.opencode)
  }
' > "$OUT_TRANSPORTS"

echo "Wrote $OUT" >&2
jq -r 'to_entries[] | "  \(.key): \(.value | length) models"' "$OUT" >&2
echo "Wrote $OUT_TRANSPORTS" >&2
jq -r 'to_entries[] | "  \(.key): \(.value | length) models"' "$OUT_TRANSPORTS" >&2
