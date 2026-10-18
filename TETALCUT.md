# TetalCut

TetalCut is AJ Media Studio's personal build of [Concat](https://github.com/jub0t/Concat)
(Jareer and the Concat contributors), licensed under the same
[AGPL-3.0-or-later](LICENSE). It is not official Concat and is not endorsed by its authors;
see [TRADEMARK.md](TRADEMARK.md).

What differs from Concat:

- The name and icon: TetalCut, its own app identifier (`com.ajmediastudio.tetalcut`), settings,
  models and new-project folder (`~/Desktop/TetalCut`), so it sits beside an official Concat
  without touching it.
- Settings ▸ Version never offers Concat's releases (one would replace TetalCut with Concat);
  TetalCut is updated from this source.
- ONNX Runtime API 23, so background removal works on Intel Macs with the last runtime built
  for them.
- `build-tetalcut.sh`: build, bundle, sign ad-hoc, make the DMG and install on this Mac.
- An iPhone build (GitHub Actions, `ios.yml`): Photos/Files import, exports saved to Photos,
  sound with the silent switch on.
- Captions as CapCut makes them: pop-in or word-by-word styles, colour templates (client
  looks included), all-caps, and a movement while on screen.
- Animation (entrance, exit, loop) for any clip or every text at once, written as keyframes.
- A keyframe button that keys every picture property at the playhead.
- Auto light and auto colour over Adjust.
- Speed curves: CapCut's ready-made ones (Montaje, Héroe, Bala, …) and a curve to draw.
- Templates: mark clips as replaceable, save the edit as a template, and make a new video from
  one by choosing the clips; on a Mac and an iPhone they live in Documents ▸ TetalCut
  Plantillas, to share.

If TetalCut is ever given or sold to anyone, the AGPL applies: its complete source, these
changes included, goes with it under the same licence.
