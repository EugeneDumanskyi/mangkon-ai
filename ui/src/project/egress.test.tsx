import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { AllowPrompt } from "../ipc/bindings/AllowPrompt";
import type { Destination } from "../ipc/bindings/Destination";
import type { ProjectEndpoint } from "../ipc/bindings/ProjectEndpoint";
import { accessLabel, purposeLabel } from "../lib/egress";
import { AllowDialog, EgressRecordsTable, ProjectEndpointRow } from "./EgressSettings";

const none = () => undefined;

const lan: Destination = {
  origin: "http://llm.lan.test:8080",
  host: "llm.lan.test",
  port: 8080,
  class: "localNetwork",
  addresses: ["192.168.1.20"],
  encrypted: false,
};

const endpoint = (over: Partial<ProjectEndpoint>): ProjectEndpoint => ({
  id: "a1",
  name: "Box",
  baseUrl: "http://llm.lan.test:8080/v1",
  model: "qwen-test",
  destination: lan,
  destinationError: null,
  access: { kind: "notAllowed" },
  ...over,
});

const row = (e: ProjectEndpoint) =>
  renderToStaticMarkup(
    <ProjectEndpointRow endpoint={e} busy={false} onAllow={none} onRevoke={none} />,
  );

describe("project AI endpoints (US-09.7)", () => {
  it("AC1: a new project's endpoint is Not allowed, with only Allow offered", () => {
    const html = row(endpoint({}));
    expect(html).toContain("Not allowed");
    expect(html).toContain("Allow for this project…");
    expect(html).not.toContain("Revoke");
  });

  it("this machine needs no permission and offers nothing", () => {
    const html = row(endpoint({ access: { kind: "thisMachine" } }));
    expect(html).toContain("This machine: no permission needed");
    expect(html).not.toContain("Allow for this project");
    expect(html).not.toContain("Revoke");
  });

  it("allowed shows the class and offers Revoke; needs reconfirmation shows the reason", () => {
    const allowed = endpoint({
      access: {
        kind: "allowed",
        class: "localNetwork",
        origin: "http://llm.lan.test:8080",
        confirmedAt: "2026-11-03T10:00:00Z",
      },
    });
    expect(accessLabel(allowed)).toMatch(/^Allowed for Local network since /);
    expect(row(allowed)).toContain("Revoke");
    expect(row(allowed)).not.toContain("Allow for this project");
    const moved = endpoint({
      access: {
        kind: "needsReconfirmation",
        reason: "llm.lan.test now points to the internet",
      },
    });
    const html = row(moved);
    expect(html).toContain(
      "Needs your confirmation again: llm.lan.test now points to the internet",
    );
    expect(html).toContain("Allow for this project…");
    expect(html).toContain("Revoke");
  });

  it("AC2: the confirm dialog names the exact host and class, Cancel first", () => {
    const prompt: AllowPrompt = {
      endpointId: "a1",
      name: "Box",
      destination: lan,
      needsConfirmation: true,
    };
    const html = renderToStaticMarkup(
      <AllowDialog prompt={prompt} error={null} busy={false} onCancel={none} onConfirm={none} />,
    );
    expect(html).toContain('role="alertdialog"');
    expect(html).toContain('data-allow-host="true">llm.lan.test:8080<');
    expect(html).toContain('data-allow-class="true">Local network<');
    expect(html).toContain("192.168.1.20");
    expect(html).toContain("Not encrypted (http)");
    expect(html.indexOf(">Cancel<")).toBeLessThan(html.indexOf(">Allow llm.lan.test"));
    expect(html).toContain("Allow llm.lan.test (Local network)");
  });
});

describe("data sent from this project (US-09.8 AC2)", () => {
  it("lists host, class, purpose and both dates", () => {
    const html = renderToStaticMarkup(
      <EgressRecordsTable
        records={[
          {
            host: "llm.lan.test",
            class: "localNetwork",
            purpose: "extraction",
            firstSentAt: "2026-11-01T09:00:00Z",
            lastSentAt: "2026-11-03T10:00:00Z",
          },
          {
            host: "127.0.0.1",
            class: "thisMachine",
            purpose: "embeddings",
            firstSentAt: "2026-11-02T09:00:00Z",
            lastSentAt: "2026-11-02T09:00:00Z",
          },
        ]}
      />,
    );
    expect(html).toContain("llm.lan.test");
    expect(html).toContain("Local network");
    expect(html).toContain("AI extraction");
    expect(html).toContain("This machine");
    expect(html).toContain("Embeddings");
    expect(html).toContain(new Date("2026-11-01T09:00:00Z").toLocaleString());
    expect(html).toContain(new Date("2026-11-03T10:00:00Z").toLocaleString());
    expect(purposeLabel("searchQuery")).toBe("Search query");
  });

  it("says plainly when nothing was sent", () => {
    expect(renderToStaticMarkup(<EgressRecordsTable records={[]} />)).toContain(
      "Nothing from this project has been sent anywhere.",
    );
  });
});
