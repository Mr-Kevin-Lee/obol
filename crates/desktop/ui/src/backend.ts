// Opened in a plain browser (e.g. Vite's dev URL) there's no backend to
// call; screens say so rather than surfacing an opaque invoke error.
export const insideApp = () => '__TAURI_INTERNALS__' in window

export const notInAppMessage =
  'Not running inside the Obol app. Launch it with `make desktop-dev`.'
