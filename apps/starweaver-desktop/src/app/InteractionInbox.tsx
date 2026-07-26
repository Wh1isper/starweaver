import { useEffect, useMemo, useState } from "react";
import type {
  ApprovalSummary,
  ClarificationAnswer,
  ClarificationSummary,
  DeferredSummary,
  useWorkspaceProduct,
} from "./useWorkspaceProduct";
import { useModalDialog } from "./useModalDialog";

type Product = ReturnType<typeof useWorkspaceProduct>;
type InteractionKind = "approval" | "clarification" | "deferred";
type InteractionSelection = { readonly kind: InteractionKind; readonly id: string };

type ClarificationDraft = Readonly<
  Record<string, { readonly freeText: string; readonly selectedOptions: readonly string[] }>
>;

function interactionTime(value: string): string {
  const date = new Date(value);
  if (Number.isNaN(date.valueOf())) return "";
  return new Intl.DateTimeFormat(undefined, {
    day: "numeric",
    hour: "numeric",
    minute: "2-digit",
    month: "short",
  }).format(date);
}

function sessionLabel(product: Product, sessionId: string): string {
  return (
    product.sessions.find((session) => session.sessionId === sessionId)?.title?.trim() ||
    "Conversation"
  );
}

function isDeferredPending(record: DeferredSummary): boolean {
  return record.status === "pending" || record.status === "running" || record.status === "waiting";
}

function pendingCount(product: Product): number {
  return (
    (product.approvals ?? []).filter((approval) => approval.status === "pending").length +
    (product.clarifications ?? []).filter((clarification) => clarification.status === "pending")
      .length +
    (product.deferred ?? []).filter(isDeferredPending).length
  );
}

export function interactionInboxCount(product: Product): number {
  return pendingCount(product);
}

function ApprovalReview({ product, approval }: { product: Product; approval: ApprovalSummary }) {
  const detail = product.approvalDetails[approval.approvalId];
  const [reason, setReason] = useState("");

  useEffect(() => {
    void product.loadApprovalDetail(approval.approvalId, approval.sessionId);
  }, [approval.approvalId, approval.sessionId, product.loadApprovalDetail]);

  return (
    <div className="interaction-review">
      <div className="interaction-review-heading">
        <p className="eyebrow">Permission request</p>
        <h3>{approval.title}</h3>
        <p>
          Review the exact requested arguments before allowing this action in the selected local
          workspace.
        </p>
      </div>
      {detail === undefined && product.approvalDetailErrors?.has(approval.approvalId) ? (
        <div className="interaction-warning" role="alert">
          <p>This durable request could not be loaded. No decision has been sent.</p>
          <button
            type="button"
            onClick={() => void product.loadApprovalDetail(approval.approvalId, approval.sessionId)}
          >
            Try again
          </button>
        </div>
      ) : detail === undefined ? (
        <p className="interaction-loading" role="status">
          Loading durable request…
        </p>
      ) : detail.argumentsComplete ? (
        <pre className="interaction-payload">{detail.argumentsJson}</pre>
      ) : (
        <p className="interaction-warning" role="alert">
          This request is too large for the safe Desktop projection. Deny it and inspect the run
          through a trusted host client if needed.
        </p>
      )}
      <label className="interaction-field">
        <span>Reason (optional)</span>
        <textarea
          value={reason}
          maxLength={2048}
          rows={2}
          onChange={(event) => setReason(event.currentTarget.value)}
        />
      </label>
      <div className="interaction-review-actions">
        <button
          type="button"
          className="interaction-secondary-action"
          disabled={product.busy}
          onClick={() => void product.decideApproval(approval, "denied", reason)}
        >
          Deny
        </button>
        <button
          type="button"
          className="interaction-primary-action"
          disabled={product.busy || detail === undefined || !detail.argumentsComplete}
          onClick={() => void product.decideApproval(approval, "approved", reason)}
        >
          Allow once
        </button>
      </div>
    </div>
  );
}

function ClarificationReview({
  product,
  clarification,
}: {
  product: Product;
  clarification: ClarificationSummary;
}) {
  const [draft, setDraft] = useState<ClarificationDraft>({});
  const answers = useMemo<readonly ClarificationAnswer[]>(
    () =>
      clarification.questions.map((question) => ({
        question: question.question,
        selectedOptions: draft[question.question]?.selectedOptions ?? [],
        ...(draft[question.question]?.freeText.trim()
          ? { freeText: draft[question.question]?.freeText.trim() }
          : {}),
      })),
    [clarification.questions, draft],
  );
  const complete = answers.every(
    (answer) => answer.selectedOptions.length > 0 || Boolean(answer.freeText),
  );

  const updateSelection = (
    question: ClarificationSummary["questions"][number],
    label: string,
    checked: boolean,
  ) => {
    setDraft((current) => {
      const existing = current[question.question] ?? { freeText: "", selectedOptions: [] };
      const selectedOptions = question.multiSelect
        ? checked
          ? [...existing.selectedOptions, label]
          : existing.selectedOptions.filter((candidate) => candidate !== label)
        : checked
          ? [label]
          : [];
      return { ...current, [question.question]: { ...existing, selectedOptions } };
    });
  };

  return (
    <form
      className="interaction-review"
      onSubmit={(event) => {
        event.preventDefault();
        if (complete) void product.resolveClarification(clarification, answers);
      }}
    >
      <div className="interaction-review-heading">
        <p className="eyebrow">Question from Starweaver</p>
        <h3>Choose how to continue</h3>
      </div>
      {clarification.questions.map((question, questionIndex) => (
        <fieldset className="clarification-question" key={question.question}>
          <legend>
            <span>{question.header}</span>
            {question.question}
          </legend>
          <div className="clarification-options">
            {question.options.map((option) => {
              const checked = (draft[question.question]?.selectedOptions ?? []).includes(
                option.label,
              );
              return (
                <label
                  key={option.label}
                  className={checked ? "option-card option-card-selected" : "option-card"}
                >
                  <input
                    type={question.multiSelect ? "checkbox" : "radio"}
                    name={`clarification-${questionIndex}`}
                    value={option.label}
                    checked={checked}
                    onChange={(event) =>
                      updateSelection(question, option.label, event.currentTarget.checked)
                    }
                  />
                  <span>
                    <strong>{option.label}</strong>
                    <small>{option.description}</small>
                    {option.preview ? <code>{option.preview}</code> : null}
                  </span>
                </label>
              );
            })}
          </div>
          <label className="interaction-field">
            <span>Additional context (optional)</span>
            <textarea
              rows={2}
              maxLength={16384}
              value={draft[question.question]?.freeText ?? ""}
              onChange={(event) =>
                setDraft((current) => ({
                  ...current,
                  [question.question]: {
                    freeText: event.currentTarget.value,
                    selectedOptions: current[question.question]?.selectedOptions ?? [],
                  },
                }))
              }
            />
          </label>
        </fieldset>
      ))}
      <div className="interaction-review-actions">
        <button
          type="submit"
          className="interaction-primary-action"
          disabled={product.busy || !complete}
        >
          Answer and continue
        </button>
      </div>
    </form>
  );
}

function DeferredReview({ product, record }: { product: Product; record: DeferredSummary }) {
  const detail = product.deferredDetails[record.deferredId];
  const [result, setResult] = useState("");
  const [error, setError] = useState("");

  useEffect(() => {
    void product.loadDeferredDetail(record.deferredId, record.sessionId);
  }, [product.loadDeferredDetail, record.deferredId, record.sessionId]);

  return (
    <div className="interaction-review">
      <div className="interaction-review-heading">
        <p className="eyebrow">External result needed</p>
        <h3>{record.toolName}</h3>
        <p>Provide the trusted result expected by this deferred tool, or fail it explicitly.</p>
      </div>
      {detail === undefined && product.deferredDetailErrors?.has(record.deferredId) ? (
        <div className="interaction-warning" role="alert">
          <p>This durable request could not be loaded. No result has been sent.</p>
          <button
            type="button"
            onClick={() => void product.loadDeferredDetail(record.deferredId, record.sessionId)}
          >
            Try again
          </button>
        </div>
      ) : detail === undefined ? (
        <p className="interaction-loading" role="status">
          Loading durable request…
        </p>
      ) : detail.requestComplete ? (
        <pre className="interaction-payload">{detail.requestJson}</pre>
      ) : (
        <p className="interaction-warning" role="alert">
          This request is too large for the safe Desktop projection. Fail it rather than supplying
          an unreviewed result.
        </p>
      )}
      <label className="interaction-field">
        <span>Result</span>
        <textarea
          rows={4}
          maxLength={65536}
          value={result}
          onChange={(event) => setResult(event.currentTarget.value)}
        />
      </label>
      <label className="interaction-field">
        <span>Failure reason</span>
        <textarea
          rows={2}
          maxLength={4096}
          value={error}
          onChange={(event) => setError(event.currentTarget.value)}
        />
      </label>
      <div className="interaction-review-actions">
        <button
          type="button"
          className="interaction-secondary-action"
          disabled={product.busy || error.trim().length === 0}
          onClick={() =>
            void product.resolveDeferred(record, { kind: "failed", error: error.trim() })
          }
        >
          Fail request
        </button>
        <button
          type="button"
          className="interaction-primary-action"
          disabled={product.busy || detail === undefined || !detail.requestComplete}
          onClick={() => void product.resolveDeferred(record, { kind: "completed", text: result })}
        >
          Submit result
        </button>
      </div>
    </div>
  );
}

export function InteractionInbox({ product, onClose }: { product: Product; onClose: () => void }) {
  const dialogRef = useModalDialog<HTMLElement>(onClose);
  const pendingApprovals = (product.approvals ?? []).filter(
    (approval) => approval.status === "pending",
  );
  const pendingClarifications = (product.clarifications ?? []).filter(
    (clarification) => clarification.status === "pending",
  );
  const pendingDeferred = (product.deferred ?? []).filter(isDeferredPending);
  const all = [
    ...pendingClarifications.map((value) => ({ kind: "clarification" as const, value })),
    ...pendingApprovals.map((value) => ({ kind: "approval" as const, value })),
    ...pendingDeferred.map((value) => ({ kind: "deferred" as const, value })),
  ].sort((left, right) => right.value.updatedAt.localeCompare(left.value.updatedAt));
  const waitingRunIds = new Set(
    product.runs.filter((run) => run.status === "waiting").map((run) => run.runId),
  );
  const pendingRunIds = new Set(all.map((entry) => entry.value.runId));
  const resumableRuns = Array.from(
    new Map(
      [
        ...product.approvals.filter((entry) => entry.status !== "pending"),
        ...product.clarifications.filter((entry) => entry.status !== "pending"),
        ...product.deferred.filter((entry) => !isDeferredPending(entry)),
      ]
        .filter((entry) => waitingRunIds.has(entry.runId) && !pendingRunIds.has(entry.runId))
        .map((entry) => [entry.runId, { runId: entry.runId, sessionId: entry.sessionId }]),
    ).values(),
  );
  const [selection, setSelection] = useState<InteractionSelection>();
  const selectionIsPending = all.some((entry) => {
    const id =
      entry.kind === "approval"
        ? entry.value.approvalId
        : entry.kind === "clarification"
          ? entry.value.clarificationId
          : entry.value.deferredId;
    return selection?.kind === entry.kind && selection.id === id;
  });
  const first = all[0];
  const effectiveSelection: InteractionSelection | undefined = selectionIsPending
    ? selection
    : first === undefined
      ? undefined
      : {
          kind: first.kind,
          id:
            first.kind === "approval"
              ? first.value.approvalId
              : first.kind === "clarification"
                ? first.value.clarificationId
                : first.value.deferredId,
        };

  const selectedApproval =
    effectiveSelection?.kind === "approval"
      ? pendingApprovals.find((approval) => approval.approvalId === effectiveSelection.id)
      : undefined;
  const selectedClarification =
    effectiveSelection?.kind === "clarification"
      ? pendingClarifications.find(
          (clarification) => clarification.clarificationId === effectiveSelection.id,
        )
      : undefined;
  const selectedDeferred =
    effectiveSelection?.kind === "deferred"
      ? pendingDeferred.find((record) => record.deferredId === effectiveSelection.id)
      : undefined;

  return (
    <aside
      ref={dialogRef}
      className="interaction-drawer"
      role="dialog"
      aria-modal="true"
      aria-labelledby="interaction-title"
      tabIndex={-1}
    >
      <header className="interaction-drawer-header">
        <div>
          <p className="eyebrow">Durable interaction inbox</p>
          <h2 id="interaction-title">Needs your attention</h2>
        </div>
        <button
          type="button"
          aria-label="Close interaction inbox"
          data-dialog-initial-focus
          onClick={onClose}
        >
          Close
        </button>
      </header>
      <div className="interaction-drawer-body">
        <nav className="interaction-list" aria-label="Pending interactions">
          <div className="interaction-list-status">
            <span>{all.length} pending</span>
            <button
              type="button"
              disabled={product.interactionsLoading}
              onClick={() => void product.refreshInteractions()}
            >
              {product.interactionsLoading ? "Refreshing…" : "Refresh"}
            </button>
          </div>
          {resumableRuns.map((run) => (
            <div className="interaction-resume-card" key={run.runId}>
              <span>Decision saved</span>
              <strong>This conversation is ready to continue.</strong>
              <button
                type="button"
                disabled={product.busy}
                onClick={() => void product.resumeResolvedInteraction(run.sessionId, run.runId)}
              >
                Resume run
              </button>
            </div>
          ))}
          {all.length === 0 && resumableRuns.length === 0 ? (
            <div className="interaction-empty">
              <strong>All clear</strong>
              <p>
                Approvals, questions, and deferred results will appear here after reconnect too.
              </p>
            </div>
          ) : (
            all.map((entry) => {
              const id =
                entry.kind === "approval"
                  ? entry.value.approvalId
                  : entry.kind === "clarification"
                    ? entry.value.clarificationId
                    : entry.value.deferredId;
              const title =
                entry.kind === "approval"
                  ? entry.value.title
                  : entry.kind === "clarification"
                    ? entry.value.questions[0]?.header || "Question"
                    : entry.value.toolName;
              const selected =
                effectiveSelection?.kind === entry.kind && effectiveSelection.id === id;
              return (
                <button
                  type="button"
                  key={`${entry.kind}:${id}`}
                  className={
                    selected ? "interaction-item interaction-item-active" : "interaction-item"
                  }
                  aria-current={selected ? "true" : undefined}
                  onClick={() => setSelection({ kind: entry.kind, id })}
                >
                  <span>{entry.kind}</span>
                  <strong>{title}</strong>
                  <small>
                    {sessionLabel(product, entry.value.sessionId)} ·{" "}
                    {interactionTime(entry.value.updatedAt)}
                  </small>
                </button>
              );
            })
          )}
        </nav>
        <section className="interaction-detail" aria-live="polite">
          {selectedApproval ? (
            <ApprovalReview
              key={selectedApproval.approvalId}
              product={product}
              approval={selectedApproval}
            />
          ) : null}
          {selectedClarification ? (
            <ClarificationReview
              key={selectedClarification.clarificationId}
              product={product}
              clarification={selectedClarification}
            />
          ) : null}
          {selectedDeferred ? (
            <DeferredReview
              key={selectedDeferred.deferredId}
              product={product}
              record={selectedDeferred}
            />
          ) : null}
          {all.length > 0 && !selectedApproval && !selectedClarification && !selectedDeferred ? (
            <div className="interaction-empty">
              <strong>This request changed</strong>
              <p>Select another pending interaction or refresh the inbox.</p>
            </div>
          ) : null}
        </section>
      </div>
    </aside>
  );
}
