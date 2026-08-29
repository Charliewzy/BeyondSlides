// Interaction for the self-contained alignment visualization.
(() => {
  const targets = [...document.querySelectorAll("[data-window-target]")];
  const details = [...document.querySelectorAll("[data-window-detail]")];

  function selectWindow(number) {
    for (const target of targets) {
      target.classList.toggle("active", target.dataset.windowTarget === number);
    }
    for (const detail of details) {
      detail.hidden = detail.dataset.windowDetail !== number;
    }
  }

  for (const target of targets) {
    target.addEventListener("click", () => selectWindow(target.dataset.windowTarget));
    target.addEventListener("keydown", event => {
      if (event.key === "Enter" || event.key === " ") {
        event.preventDefault();
        selectWindow(target.dataset.windowTarget);
      }
    });
  }

  if (targets.length > 0) {
    selectWindow(targets[0].dataset.windowTarget);
  }
})();
