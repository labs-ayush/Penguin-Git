import { render, screen, waitFor } from "@testing-library/react";
import { describe, it, expect, vi, beforeEach } from "vitest";
import ExplainCommitModal from "./ExplainCommitModal";
import ExplainBranchModal from "./ExplainBranchModal";
import PrDescriptionModal from "./PrDescriptionModal";
import * as tauriBridge from "../../services/tauriBridge";
import { setStore } from "../../test/helpers";

vi.mock("../../services/tauriBridge", () => ({
  aiExplainCommit: vi.fn(),
  aiExplainBranch: vi.fn(),
  aiGeneratePrDescription: vi.fn(),
}));

describe("AI Modals", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    localStorage.clear();
    setStore();
  });

  describe("ExplainCommitModal", () => {
    it("fetches and renders commit explanation, caching by commit hash", async () => {
      tauriBridge.aiExplainCommit.mockResolvedValue("This commit added AI support.");

      render(<ExplainCommitModal hash="abc1234" onClose={() => {}} />);

      await waitFor(() => {
        expect(screen.getByText("This commit added AI support.")).toBeInTheDocument();
      });

      expect(tauriBridge.aiExplainCommit).toHaveBeenCalledWith("/repo", "abc1234");
      expect(localStorage.getItem("penguingit_explain_commit_abc1234")).toBe(
        "This commit added AI support."
      );
    });

    it("uses cached explanation on second click without calling backend", async () => {
      localStorage.setItem("penguingit_explain_commit_abc1234", "Cached explanation text.");

      render(<ExplainCommitModal hash="abc1234" onClose={() => {}} />);

      await waitFor(() => {
        expect(screen.getByText("Cached explanation text.")).toBeInTheDocument();
        expect(screen.getByText("⚡ Cache Hit")).toBeInTheDocument();
      });

      expect(tauriBridge.aiExplainCommit).not.toHaveBeenCalled();
    });

    it("sanitizes HTML and renders markdown in explanation", async () => {
      const maliciousExplanation =
        'This is **bold** text and <script>alert("XSS")</script><iframe src="javascript:alert(1)"></iframe>.';
      tauriBridge.aiExplainCommit.mockResolvedValue(maliciousExplanation);

      render(<ExplainCommitModal hash="abc1234" onClose={() => {}} />);

      await waitFor(() => {
        expect(screen.getByText("bold").tagName).toBe("STRONG");
        const scriptElement = document.querySelector("script");
        if (scriptElement) {
          expect(scriptElement.textContent).not.toContain("XSS");
        }
        expect(document.querySelector("iframe")).toBeNull();
      });
    });
  });

  describe("ExplainBranchModal", () => {
    it("fetches branch explanation and caches by resolved tip SHA", async () => {
      tauriBridge.aiExplainBranch.mockResolvedValue({
        explanation: "Branch refactored storage.",
        branchTipSha: "sha_branch_tip_111",
        targetTipSha: "sha_target_tip_222",
      });

      render(<ExplainBranchModal branch="feature/ai" target="main" onClose={() => {}} />);

      await waitFor(() => {
        expect(screen.getByText("Branch refactored storage.")).toBeInTheDocument();
      });

      expect(tauriBridge.aiExplainBranch).toHaveBeenCalledWith("/repo", "feature/ai", "main");
      expect(
        localStorage.getItem("penguingit_explain_branch_sha_branch_tip_111_sha_target_tip_222")
      ).toBe("Branch refactored storage.");
    });
  });

  describe("PrDescriptionModal", () => {
    it("generates and displays editable PR title and body", async () => {
      tauriBridge.aiGeneratePrDescription.mockResolvedValue({
        title: "feat: Phase 5 AI Features",
        body: "## Summary\nAdded AI features.",
      });

      render(<PrDescriptionModal branch="feature/ai" target="main" onClose={() => {}} />);

      await waitFor(() => {
        expect(screen.getByDisplayValue("feat: Phase 5 AI Features")).toBeInTheDocument();
        expect(screen.getByDisplayValue(/## Summary/)).toBeInTheDocument();
      });

      expect(tauriBridge.aiGeneratePrDescription).toHaveBeenCalledWith(
        "/repo",
        "feature/ai",
        "main"
      );
    });
  });
});
