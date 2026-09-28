export function guardWheel(scope: EventTarget, area: EventTarget): void {
  let engaged = false;
  scope.addEventListener(
    "pointerdown",
    () => {
      engaged = true;
    },
    true,
  );
  scope.addEventListener("blur", () => {
    engaged = false;
  });
  area.addEventListener("mouseleave", () => {
    engaged = false;
  });
  scope.addEventListener(
    "wheel",
    (event) => {
      if (!engaged) {
        event.stopImmediatePropagation();
      }
    },
    { capture: true },
  );
}
