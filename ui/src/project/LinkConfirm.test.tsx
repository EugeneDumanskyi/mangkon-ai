import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { LinkConfirm } from "./Links";

const none = () => undefined;

// US-08.4 AC3: before a link leaves the app, the full URL is shown and the user is asked.
describe("link confirm (US-08.4 AC3)", () => {
  const url = "https://example.org/very/long/path?with=query&and=more#fragment";

  it("shows the full URL and asks, with Cancel first", () => {
    const html = renderToStaticMarkup(
      <LinkConfirm url={url} error={null} onCancel={none} onOpen={none} />,
    );
    expect(html).toContain('role="alertdialog"');
    expect(html).toContain("Open this link in your browser?");
    expect(html).toContain(url.replace(/&/g, "&amp;"));
    expect(html.indexOf(">Cancel<")).toBeLessThan(html.indexOf(">Open<"));
    expect(html).not.toMatch(/href=/i);
  });

  it("names the mail app for a mailto link", () => {
    const html = renderToStaticMarkup(
      <LinkConfirm url="mailto:a@b.example" error={null} onCancel={none} onOpen={none} />,
    );
    expect(html).toContain("Open this address in your mail app?");
  });

  it("shows why the core refused it", () => {
    const html = renderToStaticMarkup(
      <LinkConfirm url={url} error="This link cannot be opened." onCancel={none} onOpen={none} />,
    );
    expect(html).toContain('role="alert"');
    expect(html).toContain("This link cannot be opened.");
  });
});
