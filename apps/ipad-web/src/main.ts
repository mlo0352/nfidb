import "./styles.css";
import { NfidbApp } from "./app";
import { PointerDiagnostics } from "./diagnostics";

const root = document.querySelector<HTMLElement>("#app");
if (!root) {
  throw new Error("NFiDB application root is missing");
}

if (location.pathname.startsWith("/diagnostics/pointer")) {
  new PointerDiagnostics(root).start();
} else {
  const app = new NfidbApp(root);
  Object.assign(window, { __nfidbDiagnostics: () => app.diagnosticSnapshot() });
  void app.start();
}
