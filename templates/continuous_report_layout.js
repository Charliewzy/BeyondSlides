// Desktop report sizing: outer edges resize symmetrically around the centered
// reader, while the middle edge redistributes its fixed width between columns.
(() => {
  const layout = document.querySelector("[data-reader-layout].has-slides");
  const transcriptPanel = document.querySelector(".transcript-card");
  const slidePanel = document.querySelector(".slide-panel");
  const resizers = [...document.querySelectorAll("[data-reader-resizer]")];
  if (!layout || !transcriptPanel || !slidePanel || resizers.length === 0) return;

  const storageKey = "beyond-slides.reader-layout.v1";
  const desktop = window.matchMedia("(min-width: 70.001rem)");
  const rem = Number.parseFloat(getComputedStyle(document.documentElement).fontSize) || 16;
  const minimumTranscriptWidth = 24 * rem;
  const minimumSlideWidth = 24 * rem;
  const horizontalAllowance = 8 * rem;
  let saved = loadSavedLayout();
  let drag;

  function clamp(value, minimum, maximum) {
    return Math.max(minimum, Math.min(maximum, value));
  }

  function viewportMaximumWidth() {
    return Math.max(0, document.documentElement.clientWidth - 3 * rem);
  }

  function minimumReaderWidth() {
    const minimap = document.querySelector("[data-minimap-prototype]");
    const minimapWidth = minimap && !minimap.hidden ? minimap.getBoundingClientRect().width : 0;
    return minimumTranscriptWidth + minimumSlideWidth + minimapWidth + 3 * rem;
  }

  function normalizedLayout(candidate) {
    const maximumWidth = viewportMaximumWidth();
    const minimumWidth = Math.min(minimumReaderWidth(), maximumWidth);
    const readerWidth = clamp(candidate.readerWidth, minimumWidth, maximumWidth);
    const maximumSlideWidth = Math.max(
      minimumSlideWidth,
      readerWidth - minimumTranscriptWidth - 4 * rem,
    );
    return {
      readerWidth,
      slideWidth: clamp(candidate.slideWidth, minimumSlideWidth, maximumSlideWidth),
    };
  }

  function currentLayout() {
    return normalizedLayout({
      readerWidth: layout.getBoundingClientRect().width,
      slideWidth: slidePanel.getBoundingClientRect().width,
    });
  }

  function apply(candidate, persist = false) {
    if (!desktop.matches) return;
    const next = normalizedLayout(candidate);
    layout.style.setProperty("--reader-width", `${next.readerWidth}px`);
    layout.style.setProperty("--slide-panel-width", `${next.slideWidth}px`);
    saved = next;
    updateAccessibleValues(next);
    if (persist) saveLayout(next);
    window.dispatchEvent(new Event("resize"));
  }

  function proportionalOuterLayout(initial, transcriptWidth, readerWidth) {
    const normalizedReaderWidth = normalizedLayout({ ...initial, readerWidth }).readerWidth;
    const fixedChromeWidth = Math.max(
      0,
      initial.readerWidth - transcriptWidth - initial.slideWidth,
    );
    const initialContentWidth = transcriptWidth + initial.slideWidth;
    const nextContentWidth = Math.max(0, normalizedReaderWidth - fixedChromeWidth);
    const scale = initialContentWidth > 0 ? nextContentWidth / initialContentWidth : 1;
    return {
      readerWidth: normalizedReaderWidth,
      slideWidth: initial.slideWidth * scale,
    };
  }

  function updateAccessibleValues(value) {
    for (const resizer of resizers) {
      const width = resizer.dataset.readerResizer === "middle"
        ? value.slideWidth
        : value.readerWidth;
      resizer.setAttribute("aria-valuenow", String(Math.round(width)));
      resizer.setAttribute("aria-valuetext", `${Math.round(width)} 像素`);
    }
  }

  function loadSavedLayout() {
    try {
      const candidate = JSON.parse(localStorage.getItem(storageKey));
      if (Number.isFinite(candidate?.readerWidth) && Number.isFinite(candidate?.slideWidth)) {
        return candidate;
      }
    } catch (_) {
      // A report remains fully usable when browser storage is unavailable.
    }
    return undefined;
  }

  function saveLayout(value) {
    try {
      localStorage.setItem(storageKey, JSON.stringify(value));
    } catch (_) {
      // Layout persistence is optional.
    }
  }

  function resetLayout() {
    try {
      localStorage.removeItem(storageKey);
    } catch (_) {
      // Layout persistence is optional.
    }
    layout.style.removeProperty("--reader-width");
    layout.style.removeProperty("--slide-panel-width");
    saved = undefined;
    requestAnimationFrame(() => {
      updateAccessibleValues(currentLayout());
      window.dispatchEvent(new Event("resize"));
    });
  }

  function beginDrag(event, resizer) {
    if (!desktop.matches || event.button !== 0) return;
    event.preventDefault();
    const initial = currentLayout();
    drag = {
      pointerId: event.pointerId,
      kind: resizer.dataset.readerResizer,
      startX: event.clientX,
      initial,
      transcriptWidth: transcriptPanel.getBoundingClientRect().width,
      resizer,
    };
    resizer.setPointerCapture(event.pointerId);
    resizer.classList.add("dragging");
    document.body.classList.add("reader-layout-dragging");
  }

  function continueDrag(event) {
    if (!drag || event.pointerId !== drag.pointerId) return;
    const delta = event.clientX - drag.startX;
    if (drag.kind === "middle") {
      apply({ ...drag.initial, slideWidth: drag.initial.slideWidth - delta });
      return;
    }
    const outwardDelta = drag.kind === "left" ? -delta : delta;
    apply(proportionalOuterLayout(
      drag.initial,
      drag.transcriptWidth,
      drag.initial.readerWidth + 2 * outwardDelta,
    ));
  }

  function finishDrag(event) {
    if (!drag || event.pointerId !== drag.pointerId) return;
    drag.resizer.classList.remove("dragging");
    document.body.classList.remove("reader-layout-dragging");
    drag = undefined;
    saveLayout(currentLayout());
  }

  function resizeFromKeyboard(event, resizer) {
    if (!desktop.matches || !["ArrowLeft", "ArrowRight"].includes(event.key)) return;
    event.preventDefault();
    const direction = event.key === "ArrowRight" ? 1 : -1;
    const step = event.shiftKey ? 32 : 12;
    const current = currentLayout();
    if (resizer.dataset.readerResizer === "middle") {
      apply({ ...current, slideWidth: current.slideWidth - direction * step }, true);
    } else {
      const outwardDirection = resizer.dataset.readerResizer === "left" ? -direction : direction;
      apply(proportionalOuterLayout(
        current,
        transcriptPanel.getBoundingClientRect().width,
        current.readerWidth + 2 * outwardDirection * step,
      ), true);
    }
  }

  for (const resizer of resizers) {
    resizer.addEventListener("pointerdown", event => beginDrag(event, resizer));
    resizer.addEventListener("pointermove", continueDrag);
    resizer.addEventListener("pointerup", finishDrag);
    resizer.addEventListener("pointercancel", finishDrag);
    resizer.addEventListener("keydown", event => resizeFromKeyboard(event, resizer));
    resizer.addEventListener("dblclick", resetLayout);
  }

  function respondToViewport() {
    if (!desktop.matches) return;
    apply(saved || currentLayout());
  }

  desktop.addEventListener("change", respondToViewport);
  window.addEventListener("resize", () => {
    if (desktop.matches && saved?.readerWidth > viewportMaximumWidth()) apply(saved);
  });
  requestAnimationFrame(() => apply(saved || currentLayout()));
})();
