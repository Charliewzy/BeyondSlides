// PROTOTYPE: one binary lecture minimap enabled by ?prototype=minimap.
(() => {
  const parameters = new URLSearchParams(window.location.search);
  if (parameters.get("prototype") !== "minimap") return;

  const passages = [...document.querySelectorAll("[data-passage]")];
  const transcript = document.querySelector(".continuous-transcript");
  const minimap = document.querySelector("[data-minimap-prototype]");
  const stage = document.querySelector("[data-minimap-stage]");
  const bandsElement = document.querySelector("[data-minimap-bands]");
  const markersElement = document.querySelector("[data-minimap-markers]");
  const viewportElement = document.querySelector("[data-minimap-viewport]");
  const tooltip = document.querySelector("[data-minimap-tooltip]");
  if (!transcript || !minimap || !stage || passages.length === 0) return;

  let transcriptTop = 0;
  let transcriptHeight = 1;
  let passagePositions = new Map();
  let bandsByPassage = new Map();
  let hoveredPassage;
  let rebuildFrame;
  let viewportFrame;
  let stateFrame;
  let mapPointer;
  let viewportDrag;

  document.body.classList.add("minimap-prototype-active");
  document.body.dataset.minimapVariant = "C";
  minimap.hidden = false;

  function clamp(value, minimum, maximum) {
    return Math.max(minimum, Math.min(maximum, value));
  }

  function passageState(passage) {
    return {
      importance: passage.classList.contains("importance-emphasized"),
      novelty: passage.classList.contains("novelty-emphasized"),
    };
  }

  function stateName(state) {
    if (state.importance && state.novelty) return "重要且新颖";
    if (state.importance) return "重要";
    if (state.novelty) return "新颖";
    return "未突出";
  }

  function passageDescription(passage) {
    const state = passageState(passage);
    return {
      title: `${passage.dataset.time} · ${stateName(state)}`,
      lines: [
        `重要性 ${passage.dataset.importance} · 新颖度 ${passage.dataset.novelty}`,
        passage.textContent.trim(),
      ],
    };
  }

  function showTooltip(passage, target) {
    const description = passageDescription(passage);
    tooltip.replaceChildren();
    const title = document.createElement("strong");
    title.textContent = description.title;
    tooltip.append(title);
    for (const line of description.lines) {
      const text = document.createElement("span");
      text.textContent = line;
      tooltip.append(text);
    }
    tooltip.hidden = false;
    const targetBounds = target.getBoundingClientRect();
    const tooltipBounds = tooltip.getBoundingClientRect();
    const left = clamp(
      targetBounds.left - tooltipBounds.width - 8,
      8,
      window.innerWidth - tooltipBounds.width - 8,
    );
    const top = clamp(
      targetBounds.top + targetBounds.height / 2 - tooltipBounds.height / 2,
      8,
      window.innerHeight - tooltipBounds.height - 8,
    );
    tooltip.style.left = `${left}px`;
    tooltip.style.top = `${top}px`;
  }

  function hideTooltip() {
    tooltip.hidden = true;
  }

  function previewPassage(passage) {
    if (hoveredPassage === passage) return;
    hoveredPassage?.classList.remove("minimap-hover-preview");
    hoveredPassage = passage;
    hoveredPassage?.classList.add("minimap-hover-preview");
  }

  function clearPassagePreview(passage) {
    if (hoveredPassage !== passage) return;
    passage.classList.remove("minimap-hover-preview");
    hoveredPassage = undefined;
  }

  function bandContent() {
    const content = document.createElement("span");
    content.className = "minimap-binary-signals";
    content.innerHTML = "<i class=\"importance\"></i><b></b><i class=\"novelty\"></i>";
    return content;
  }

  function updateBandState(passage, band) {
    const state = passageState(passage);
    band.dataset.importanceActive = String(state.importance);
    band.dataset.noveltyActive = String(state.novelty);
    const description = passageDescription(passage);
    band.setAttribute(
      "aria-label",
      `${description.title}。${description.lines[0]}。点击定位讲稿。`,
    );
  }

  function updateBinaryStates() {
    stateFrame = undefined;
    for (const [passage, band] of bandsByPassage) {
      updateBandState(passage, band);
    }
    updateMarkers();
  }

  function scheduleStateUpdate() {
    if (stateFrame === undefined) stateFrame = requestAnimationFrame(updateBinaryStates);
  }

  function passageGeometry(passage) {
    const rectangles = [...passage.getClientRects()]
      .filter(bounds => bounds.width > 0 && bounds.height > 0);
    if (rectangles.length === 0) return;
    const top = Math.min(...rectangles.map(bounds => bounds.top + window.scrollY));
    const bottom = Math.max(...rectangles.map(bounds => bounds.bottom + window.scrollY));
    return {
      top: clamp((top - transcriptTop) / transcriptHeight, 0, 1),
      bottom: clamp((bottom - transcriptTop) / transcriptHeight, 0, 1),
    };
  }

  function rebuild() {
    rebuildFrame = undefined;
    hideTooltip();
    previewPassage(undefined);
    const transcriptBounds = transcript.getBoundingClientRect();
    transcriptTop = transcriptBounds.top + window.scrollY;
    transcriptHeight = Math.max(1, transcriptBounds.height);
    passagePositions = new Map();
    bandsByPassage = new Map();
    bandsElement.replaceChildren();

    for (const passage of passages) {
      const geometry = passageGeometry(passage);
      if (!geometry) continue;
      const band = document.createElement("button");
      band.type = "button";
      band.className = "minimap-prototype-passage";
      band.style.top = `${geometry.top * 100}%`;
      band.style.height = `${Math.max(.0015, geometry.bottom - geometry.top) * 100}%`;
      band.append(bandContent());
      updateBandState(passage, band);
      band.addEventListener("mouseenter", () => {
        previewPassage(passage);
        showTooltip(passage, band);
      });
      band.addEventListener("mouseleave", () => {
        clearPassagePreview(passage);
        hideTooltip();
      });
      band.addEventListener("focus", () => {
        previewPassage(passage);
        showTooltip(passage, band);
      });
      band.addEventListener("blur", () => {
        clearPassagePreview(passage);
        hideTooltip();
      });
      band.addEventListener("click", event => {
        event.stopPropagation();
        passage.scrollIntoView({ behavior: "smooth", block: "center" });
      });
      passagePositions.set(passage, (geometry.top + geometry.bottom) / 2);
      bandsByPassage.set(passage, band);
      bandsElement.append(band);
    }

    updateViewport();
    updateMarkers();
  }

  function scheduleRebuild() {
    if (rebuildFrame === undefined) rebuildFrame = requestAnimationFrame(rebuild);
  }

  function updateViewport() {
    viewportFrame = undefined;
    const visibleTop = clamp((window.scrollY - transcriptTop) / transcriptHeight, 0, 1);
    const visibleBottom = clamp(
      (window.scrollY + window.innerHeight - transcriptTop) / transcriptHeight,
      0,
      1,
    );
    const height = Math.max(.008, visibleBottom - visibleTop);
    viewportElement.style.top = `${Math.min(1 - height, visibleTop) * 100}%`;
    viewportElement.style.height = `${height * 100}%`;
  }

  function scheduleViewportUpdate() {
    if (viewportFrame === undefined) viewportFrame = requestAnimationFrame(updateViewport);
  }

  function addMarker(passage, kind) {
    const position = passagePositions.get(passage);
    if (position === undefined) return;
    const marker = document.createElement("i");
    marker.className = `minimap-prototype-marker ${kind}`;
    marker.style.top = `${position * 100}%`;
    markersElement.append(marker);
  }

  function updateMarkers() {
    markersElement.replaceChildren();
    for (const passage of passages) {
      if (passage.classList.contains("slide-aligned")) addMarker(passage, "aligned");
    }
    const selected = passages.find(passage => passage.classList.contains("selected"));
    const currentAudio = passages.find(passage => passage.classList.contains("audio-current"));
    if (selected) addMarker(selected, "selected");
    if (currentAudio) addMarker(currentAudio, "audio");
  }

  function scrollToRatio(ratio) {
    const target = transcriptTop + clamp(ratio, 0, 1) * transcriptHeight - window.innerHeight / 2;
    window.scrollTo({ top: target, behavior: "auto" });
  }

  function scrollFromPointer(event) {
    const bounds = stage.getBoundingClientRect();
    scrollToRatio((event.clientY - bounds.top) / bounds.height);
  }

  stage.addEventListener("pointerdown", event => {
    if (event.button !== 0 || event.target.closest(".minimap-prototype-passage")) return;
    mapPointer = event.pointerId;
    stage.setPointerCapture(event.pointerId);
    scrollFromPointer(event);
  });
  stage.addEventListener("pointermove", event => {
    if (event.pointerId === mapPointer) scrollFromPointer(event);
  });
  stage.addEventListener("pointerup", event => {
    if (event.pointerId === mapPointer) mapPointer = undefined;
  });
  stage.addEventListener("pointercancel", event => {
    if (event.pointerId === mapPointer) mapPointer = undefined;
  });
  stage.addEventListener("keydown", event => {
    if (event.key !== "ArrowUp" && event.key !== "ArrowDown") return;
    event.preventDefault();
    const direction = event.key === "ArrowUp" ? -1 : 1;
    window.scrollBy({ top: direction * window.innerHeight * .75, behavior: "auto" });
  });

  viewportElement.addEventListener("pointerdown", event => {
    if (event.button !== 0) return;
    event.stopPropagation();
    viewportDrag = {
      pointerId: event.pointerId,
      pointerY: event.clientY,
      scrollY: window.scrollY,
    };
    viewportElement.setPointerCapture(event.pointerId);
    viewportElement.classList.add("dragging");
  });
  viewportElement.addEventListener("pointermove", event => {
    if (event.pointerId !== viewportDrag?.pointerId) return;
    const stageHeight = stage.getBoundingClientRect().height;
    const scrollDelta = (event.clientY - viewportDrag.pointerY) / stageHeight * transcriptHeight;
    window.scrollTo({ top: viewportDrag.scrollY + scrollDelta, behavior: "auto" });
  });
  function endViewportDrag(event) {
    if (event.pointerId !== viewportDrag?.pointerId) return;
    viewportDrag = undefined;
    viewportElement.classList.remove("dragging");
  }
  viewportElement.addEventListener("pointerup", endViewportDrag);
  viewportElement.addEventListener("pointercancel", endViewportDrag);
  viewportElement.addEventListener("lostpointercapture", endViewportDrag);

  window.addEventListener("scroll", scheduleViewportUpdate, { passive: true });
  window.addEventListener("resize", scheduleRebuild);

  const passageObserver = new MutationObserver(scheduleStateUpdate);
  for (const passage of passages) {
    passageObserver.observe(passage, { attributes: true, attributeFilter: ["class"] });
  }
  new ResizeObserver(scheduleRebuild).observe(transcript);
  document.fonts?.ready.then(scheduleRebuild);
  rebuild();
})();
