// Test-page behaviour (external file: no inline scripts).
(async () => {
  // 2. blob: video — fetch the file and play it from an object URL.
  const resp = await fetch("/media/blob-source.mp4");
  const blob = await resp.blob();
  const v2 = document.getElementById("v2");
  v2.src = URL.createObjectURL(blob);
  v2.play().catch(() => {});
})();

// 1. start playing so the floating button's "playing" rule applies.
document.getElementById("v1").play().catch(() => {});

// 3. insert a video after 3 seconds.
setTimeout(() => {
  const v = document.createElement("video");
  v.controls = true;
  v.muted = true;
  v.loop = true;
  v.width = 640;
  v.height = 360;
  v.src = "/media/dynamic.mp4";
  const slot = document.getElementById("dyn");
  slot.replaceChildren(v);
  v.play().catch(() => {});
}, 3000);
