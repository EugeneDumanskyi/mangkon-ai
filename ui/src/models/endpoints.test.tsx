import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { EndpointItem } from "../ipc/bindings/EndpointItem";
import type { EndpointTestReport } from "../ipc/bindings/EndpointTestReport";
import { NO_STRUCTURED_OUTPUT, destinationLine, keyInput } from "../lib/egress";
import { EndpointForm, EndpointRowView, SessionNotice, TestReportView } from "./ModelsScreen";

const none = () => undefined;

const endpoint = (over: Partial<EndpointItem>): EndpointItem => ({
  id: "a1",
  name: "Box",
  baseUrl: "http://llm.lan.test:8080/v1",
  model: "qwen-test",
  key: "stored",
  structuredOutput: null,
  lastTest: null,
  destination: {
    origin: "http://llm.lan.test:8080",
    host: "llm.lan.test",
    port: 8080,
    class: "localNetwork",
    addresses: ["192.168.1.20"],
    encrypted: false,
  },
  destinationError: null,
  ...over,
});

const report = (over: Partial<EndpointTestReport>): EndpointTestReport => ({
  ok: true,
  summary: "Connected: qwen-test answered in 42 ms",
  origin: "http://llm.lan.test:8080",
  addresses: ["192.168.1.20"],
  class: "localNetwork",
  reportedModel: "qwen-test",
  responseMs: 42,
  errorCode: null,
  structuredOutput: true,
  embeddingsDimensions: 768,
  embeddingsError: null,
  ...over,
});

const row = (e: EndpointItem, r: EndpointTestReport | null = null) =>
  renderToStaticMarkup(
    <EndpointRowView
      endpoint={e}
      testing={false}
      report={r}
      onTest={none}
      onEdit={none}
      onDelete={none}
    />,
  );

describe("AI models: endpoints (US-09.6)", () => {
  it("AC1: shows whether a key is stored, never the key, and the class", () => {
    const html = row(endpoint({}));
    expect(html).toContain("API key in the keychain");
    expect(html).toContain("llm.lan.test:8080 · Local network · 192.168.1.20");
    expect(html).toContain("Not encrypted (http)");
    expect(row(endpoint({ key: "missing" }))).toContain("API key missing: enter it again");
    expect(row(endpoint({ key: "none" }))).toContain("No API key");
  });

  it("AC1: the key field is a password field, empty, and the form says where keys go", () => {
    const add = renderToStaticMarkup(
      <EndpointForm
        editing={null}
        destination={null}
        destinationError={null}
        saving={false}
        error={null}
        onCheck={none}
        onSave={none}
        onCancel={none}
      />,
    );
    expect(add).toMatch(/type="password"[^>]*value=""/);
    expect(add).toContain("Kept in the system keychain, never in a file.");
    expect(add).not.toContain("Remove the stored key");
    const edit = renderToStaticMarkup(
      <EndpointForm
        editing={endpoint({})}
        destination={endpoint({}).destination}
        destinationError={null}
        saving={false}
        error={null}
        onCheck={none}
        onSave={none}
        onCancel={none}
      />,
    );
    expect(edit).toContain("Leave empty to keep it");
    expect(edit).toContain("Remove the stored key");
    expect(edit).toContain("Goes to llm.lan.test:8080 · Local network");
  });

  it("AC1: what happens to the key on save", () => {
    expect(keyInput("sk-1", false, true)).toEqual({ kind: "set", value: "sk-1" });
    expect(keyInput("", false, true)).toEqual({ kind: "keep" });
    expect(keyInput("", true, true)).toEqual({ kind: "remove" });
    expect(keyInput("", false, false)).toEqual({ kind: "remove" });
  });

  it("AC2: the test shows the model and time, or the plain error", () => {
    const ok = renderToStaticMarkup(<TestReportView report={report({})} />);
    expect(ok).toContain("Connected: qwen-test answered in 42 ms");
    expect(ok).toContain("768 dimensions");
    const failed = renderToStaticMarkup(
      <TestReportView
        report={report({
          ok: false,
          summary: "The endpoint refused the API key",
          errorCode: "key_refused",
          reportedModel: null,
          responseMs: null,
          structuredOutput: null,
          embeddingsDimensions: null,
        })}
      />,
    );
    expect(failed).toContain("The endpoint refused the API key");
    expect(failed).not.toContain("dimensions");
  });

  it("AC3: an endpoint without structured output carries the warning", () => {
    expect(row(endpoint({ structuredOutput: false }))).toContain(NO_STRUCTURED_OUTPUT);
    expect(row(endpoint({ structuredOutput: true }))).not.toContain(NO_STRUCTURED_OUTPUT);
    expect(row(endpoint({ structuredOutput: null }))).not.toContain(NO_STRUCTURED_OUTPUT);
  });

  it("ADR-0012: the session-only notice shows only without a keychain", () => {
    const notice = "API keys are kept only until Mangkon AI quits.";
    expect(renderToStaticMarkup(<SessionNotice info={{ keyring: false, notice }} />)).toContain(
      notice,
    );
    expect(renderToStaticMarkup(<SessionNotice info={{ keyring: true, notice: null }} />)).toBe("");
  });

  it("an https destination on the internet has no warning", () => {
    expect(
      destinationLine({
        origin: "https://api.example.test:443",
        host: "api.example.test",
        port: 443,
        class: "internet",
        addresses: ["93.184.215.14"],
        encrypted: true,
      }),
    ).toBe("api.example.test:443 · Internet · 93.184.215.14");
  });
});
