const panel = document.querySelector("#technical-panel");
const openButton = document.querySelector("[data-panel-open]");

if (panel && openButton) {
  openButton.addEventListener("click", () => panel.showModal());
  panel.addEventListener("click", (event) => {
    if (event.target === panel) panel.close();
  });
}
