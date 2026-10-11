# Mermoz launch video

Animated product video (Remotion), with fictional demo content only.

```
npm install
npm run studio            # preview and tweak
npm run render            # out/mermoz-launch.mp4          1920×1080, 36.5 s
npm run render:vertical   # out/mermoz-launch-vertical.mp4 1080×1920
```

Scenes live in `src/scenes.tsx`, the visual language in `src/ui.tsx`, timing in
`src/Root.tsx`. The video is silent: add music in the editor of the platform you
post to.
