(() => {
  const passages = [...document.querySelectorAll("[data-passage]")];
  const range = document.querySelector("[data-inspector-range]");
  const details = document.querySelector("[data-inspector-details]");
  const rail = document.querySelector("[data-slide-rail]");
  const slides = [...document.querySelectorAll("[data-slide]")];
  const slidesById = new Map(slides.map(slide => [slide.dataset.slideId, slide]));
  const alignedSlideLabel = document.querySelector("[data-aligned-slide-label]");
  const viewingSlideLabel = document.querySelector("[data-viewing-slide-label]");
  const audio = document.querySelector("[data-lecture-audio]");
  const audioStatus = document.querySelector("[data-audio-status]");
  let viewingFrame;
  let activeAudioEnd;

  function updateViewingSlide() {
    viewingFrame = undefined;
    if (!rail || slides.length === 0) return;

    const railBounds = rail.getBoundingClientRect();
    const railCenter = railBounds.top + railBounds.height / 2;
    let viewingSlide = slides[0];
    let closestDistance = Number.POSITIVE_INFINITY;

    for (const slide of slides) {
      const bounds = slide.getBoundingClientRect();
      const distance = Math.abs(bounds.top + bounds.height / 2 - railCenter);
      if (distance < closestDistance) {
        viewingSlide = slide;
        closestDistance = distance;
      }
    }

    for (const slide of slides) {
      slide.classList.toggle("viewing", slide === viewingSlide);
    }
    viewingSlideLabel.textContent = `浏览页 ${viewingSlide.dataset.slideNumber}`;
  }

  function scheduleViewingSlideUpdate() {
    if (viewingFrame === undefined) {
      viewingFrame = requestAnimationFrame(updateViewingSlide);
    }
  }

  function centerSlide(slide, behavior) {
    const railBounds = rail.getBoundingClientRect();
    const slideBounds = slide.getBoundingClientRect();
    rail.scrollTo({
      top: rail.scrollTop + slideBounds.top - railBounds.top
        - (rail.clientHeight - slideBounds.height) / 2,
      behavior,
    });
    scheduleViewingSlideUpdate();
  }

  function playPassageAudio(passage) {
    if (!audio) return;
    const start = Number(passage.dataset.audioStartMs) / 1000;
    const end = Number(passage.dataset.audioEndMs) / 1000;
    if (!Number.isFinite(start) || !Number.isFinite(end) || end <= start) return;

    activeAudioEnd = end;
    audio.currentTime = start;
    audioStatus.textContent = `正在播放 ${passage.dataset.time}`;
    const playback = audio.play();
    if (playback) {
      playback.catch(() => {
        audioStatus.textContent = `音频已定位到 ${passage.dataset.time}；点击播放按钮开始`;
      });
    }
  }

  function inspectPassage(passage, scrollBehavior = "smooth", playAudio = false) {
    for (const candidate of passages) {
      candidate.classList.toggle("selected", candidate === passage);
    }
    range.textContent = `来源片段 #${passage.dataset.sourceStart}–${passage.dataset.sourceEnd} · ${passage.dataset.time}`;
    details.textContent = `对齐页 ${passage.dataset.slideNumber} · 重要性 ${passage.dataset.importance} · 新颖度 ${passage.dataset.novelty} · 连接强度 ${passage.dataset.connection}`;

    if (playAudio) playPassageAudio(passage);

    if (!rail) return;
    const alignedSlide = slidesById.get(passage.dataset.slidePosition);
    if (!alignedSlide) return;
    for (const slide of slides) {
      const aligned = slide === alignedSlide;
      slide.classList.toggle("aligned", aligned);
      if (aligned) {
        slide.setAttribute("aria-current", "true");
      } else {
        slide.removeAttribute("aria-current");
      }
    }
    alignedSlideLabel.textContent = `对齐页 ${alignedSlide.dataset.slideNumber}`;
    centerSlide(alignedSlide, scrollBehavior);
  }

  for (const passage of passages) {
    passage.addEventListener("click", () => inspectPassage(passage, "smooth", true));
    passage.addEventListener("keydown", event => {
      if (event.key === "Enter" || event.key === " ") {
        event.preventDefault();
        inspectPassage(passage, "smooth", true);
      }
    });
  }

  if (rail) {
    rail.addEventListener("scroll", scheduleViewingSlideUpdate, { passive: true });
    window.addEventListener("resize", scheduleViewingSlideUpdate);
  }
  if (audio) {
    audio.addEventListener("timeupdate", () => {
      if (activeAudioEnd !== undefined && audio.currentTime >= activeAudioEnd) {
        audio.pause();
        audio.currentTime = activeAudioEnd;
        activeAudioEnd = undefined;
        audioStatus.textContent = "已播放所选讲稿段落的音频";
      }
    });
  }
  if (passages.length > 0) {
    inspectPassage(passages[0], "auto");
  } else {
    updateViewingSlide();
  }
})();
