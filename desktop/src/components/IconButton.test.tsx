import { render, screen } from "@testing-library/react";
import { Settings } from "lucide-react";
import { afterEach, describe, expect, it } from "vitest";
import "../styles/global.css";
import { changeInterfaceLanguage } from "../i18n";
import { IconButton } from "./IconButton";

describe("IconButton", () => {
  afterEach(async () => {
    await changeInterfaceLanguage("en");
  });

  it("anchors its tooltip at the inline end in RTL", async () => {
    await changeInterfaceLanguage("ar-XB");
    render(<IconButton icon={Settings} label="Open settings" tooltip="Settings" />);

    const styles = getComputedStyle(screen.getByRole("tooltip"));

    expect(styles.insetInlineEnd).toBe("0px");
    expect(styles.right).toBe("auto");
  });
});
