import { screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { mountApp } from "../helpers";

vi.mock("../../src/shell/TitleBar", () => ({
  TitleBar: () => {
    throw new Error("title bar blew up");
  },
}));

let logged: ReturnType<typeof vi.spyOn>;
beforeEach(() => {
  logged = vi.spyOn(console, "error").mockImplementation(() => undefined);
});
afterEach(() => logged.mockRestore());

describe("the app-level boundary", () => {
  it("a failure in the shell itself leaves an explanation and a reload, not a blank window", () => {
    mountApp("/inbox");
    const fallback = screen.getByTestId("boundary-app");
    expect(fallback).toHaveTextContent("Marsad hit an error and cannot show this window");
    expect(fallback).toHaveTextContent("title bar blew up");
    expect(fallback).toHaveTextContent("Your sessions keep running");
    expect(screen.getByRole("button", { name: "Reload the window" })).toBeInTheDocument();
  });
});
