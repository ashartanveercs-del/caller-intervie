import { createBrowserPlatform } from "./browser";
import { createDesktopPlatform } from "./desktop";
import type { PlatformApi } from "./types";

export * from "./browser";
export * from "./desktop";
export * from "./types";

export function createPlatform(): PlatformApi {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window
    ? createDesktopPlatform()
    : createBrowserPlatform();
}
