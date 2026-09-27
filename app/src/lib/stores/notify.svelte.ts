import { errorMessage } from "../api";

export type NoticeKind = "info" | "error";

class Notices {
  current = $state<{ text: string; kind: NoticeKind } | null>(null);
  #timer: ReturnType<typeof setTimeout> | undefined;

  show(text: string, kind: NoticeKind = "info") {
    this.current = { text, kind };
    clearTimeout(this.#timer);
    this.#timer = setTimeout(() => (this.current = null), kind === "error" ? 9000 : 5000);
  }

  error(e: unknown) {
    console.error(e);
    this.show(errorMessage(e), "error");
  }
}

export const notices = new Notices();
