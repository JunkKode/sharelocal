import { isTauri } from "./platform";
import { DesktopApp } from "./views/DesktopApp";
import { BrowserApp } from "./views/BrowserApp";

/**
 * One bundle, two surfaces: the Tauri window gets the full desktop app, a
 * phone that scanned the QR code gets the browser client.
 */
export default function App() {
  return isTauri ? <DesktopApp /> : <BrowserApp />;
}
