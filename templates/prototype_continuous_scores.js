// THROWAWAY UI PROTOTYPE — no persistence or production contract.
(() => {
  const variants = new Set(["editorial", "signal", "manuscript"]);
  const buttons = [...document.querySelectorAll("[data-variant-button]")];
  const passages = [...document.querySelectorAll("[data-passage]")];
  const inspector = document.querySelector("[data-passage-inspector]");

  function selectVariant(variant, updateUrl = true) {
    const selected = variants.has(variant) ? variant : "editorial";
    document.documentElement.dataset.variant = selected;
    for (const button of buttons) {
      button.classList.toggle("active", button.dataset.variantButton === selected);
    }
    if (updateUrl) {
      const url = new URL(window.location.href);
      url.searchParams.set("variant", selected);
      history.replaceState(null, "", url);
    }
  }

  function inspectPassage(passage) {
    for (const candidate of passages) {
      candidate.classList.toggle("selected", candidate === passage);
    }
    inspector.innerHTML = `<p><strong>#${passage.dataset.start}–${passage.dataset.end}</strong> · ${passage.dataset.time}<br>重要性 ${passage.dataset.importance} · 新颖度 ${passage.dataset.novelty} · 连接强度 ${passage.dataset.connection}</p>`;
  }

  for (const button of buttons) {
    button.addEventListener("click", () => selectVariant(button.dataset.variantButton));
  }
  for (const passage of passages) {
    passage.addEventListener("click", () => inspectPassage(passage));
    passage.addEventListener("keydown", event => {
      if (event.key === "Enter" || event.key === " ") {
        event.preventDefault();
        inspectPassage(passage);
      }
    });
  }

  selectVariant(new URL(window.location.href).searchParams.get("variant"), false);
})();
