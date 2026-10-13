import { getCurrentWindow } from "@tauri-apps/api/window";
import { HomeApp } from "./app/HomeApp";
import { OverlayApp } from "./app/OverlayApp";

function windowLabel(): string {
  try {
    return getCurrentWindow().label;
  } catch {
    return "main";
  }
}

export default function App() {
  return windowLabel().startsWith("overlay-") ? <OverlayApp /> : <HomeApp />;
}
