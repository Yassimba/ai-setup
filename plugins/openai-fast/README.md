# @yassimba/pi-openai-fast

A small Pi extension that lets you turn on OpenAI Fast Mode from inside Pi.

When Fast Mode is active, the extension asks OpenAI for the `priority` service tier on supported OpenAI models and shows a simple `fast` indicator in Pi.

Fast Mode also follows newly launched Pi subagents, so workers, reviewers, scouts, and other child sessions can use the same Fast Mode preference automatically.

## Install

```bash
pi install npm:@yassimba/pi-openai-fast
```

## Use it

Inside Pi, run:

```text
/fast
```

That toggles Fast Mode on or off for your current Pi session.

You can also start a Pi run with Fast Mode already requested:

```bash
pi --fast
```

## Works with subagents

Turn on `/fast` once in your parent Pi session, then launch subagents as usual. Newly started subagents inherit the same Fast Mode preference.

That means a subagent can request OpenAI priority too, as long as it is using a supported model and has this extension available.

For a quick sanity check, ask a subagent to print `PI_OPENAI_FAST_DESIRED`. A value of `1` means Fast Mode was handed off to that subagent.

## How it behaves

Fast Mode only turns on when both of these are true:

1. You have requested Fast Mode with `/fast` or `--fast`.
2. Your current Pi model is in the supported-model list.

If you turn Fast Mode on while using an unsupported model, Pi keeps your preference but does not send priority requests until you switch to a supported model.

By default, `/fast` is session-only. If you want Pi to remember your choice between runs, enable persistence in the config below.

## Supported models

Fast Mode is currently supported by these Pi model keys:

```json
[
  "openai/gpt-5.4",
  "openai/gpt-5.5",
  "openai-codex/gpt-5.4",
  "openai-codex/gpt-5.5",
  "openai-codex/gpt-5.6-sol",
  "openai-codex/gpt-5.6-terra",
  "openai-codex/gpt-5.6-luna"
]
```

You can change this list in your config. Model names must match Pi's `provider/model` key exactly.

## Configure it

Config files can live in either place:

- Global: `~/.pi/agent/extensions/pi-openai-fast.json`
- Project: `.pi/extensions/pi-openai-fast.json`

Project config overrides global config.

Example config:

```json
{
  "persistState": true,
  "desiredActive": true,
  "supportedModels": [
    "openai/gpt-5.4",
    "openai/gpt-5.5",
    "openai-codex/gpt-5.4",
    "openai-codex/gpt-5.5",
    "openai-codex/gpt-5.6-sol",
    "openai-codex/gpt-5.6-terra",
    "openai-codex/gpt-5.6-luna"
  ],
  "footer": {
    "mode": "replace"
  }
}
```

Common options:

- `persistState`: set to `true` if `/fast` should be remembered between Pi runs.
- `desiredActive`: the saved on/off preference used when `persistState` is `true`.
- `supportedModels`: exact model keys that may use Fast Mode.
- `footer.mode`:
  - `replace` shows `fast` next to the model name in Pi's footer. This is the default.
  - `status` leaves Pi's normal footer alone and shows a plain status indicator.
  - `off` hides Fast Mode UI feedback. Priority requests still work when Fast Mode is active.

## Customize the `fast` label color

In the default `replace` footer mode, the `fast` label follows your Pi theme automatically.

If you want custom colors, set one or both of these fields:

```json
{
  "footer": {
    "mode": "replace",
    "darkFastColor": "#00ffaa",
    "lightFastColor": "#0066cc"
  }
}
```

If a custom color is invalid or missing, Pi falls back to the theme-matched color.

## Troubleshooting

**I turned Fast Mode on, but I do not see `fast`.**

Check that your current model is in `supportedModels`. Also check that `footer.mode` is not set to `off`.

**My `/fast` choice is not remembered after restarting Pi.**

Set `"persistState": true` in your config.

**My subagent does not seem to be using Fast Mode.**

Subagents only inherit Fast Mode when they start after `/fast` is turned on. The subagent also needs to use a supported model. To debug, ask it to print `PI_OPENAI_FAST_DESIRED`; `1` means the preference was inherited.

**I see a config warning.**

Check that your JSON is valid and that every entry in `supportedModels` is an exact `provider/model` key.

**Does this guarantee faster responses?**

No. The extension requests OpenAI's priority service tier when possible. Actual availability, latency, and billing behavior depend on OpenAI and your account.

## Attribution

This package is based on [studioarray/pi-openai-fast](https://github.com/studioarray/pi-openai-fast), commit [`e82ed32`](https://github.com/studioarray/pi-openai-fast/commit/e82ed32f1b7c5a946d441d948da33de40da7b04a), under the MIT License.

The upstream project was inspired by [pi-better-openai](https://github.com/mattleong/pi-better-openai/).
