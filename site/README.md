# mermoz.xyz

The landing page: plain HTML, CSS and a little JavaScript, no build step.

- `index.html`, `style.css`, `main.js` (the paper-plane sky).
- `assets/`: icon, the launch film (from `marketing/video`), poster and share image.
- Preview: `python3 -m http.server -d site 8000`, then open http://localhost:8000.
- Deploy: pushing to `main` publishes it with GitHub Pages
  (`.github/workflows/site.yml`); `CNAME` sets the domain to mermoz.xyz.
