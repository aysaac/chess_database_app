import { api, onJob } from "../api";
import type { JobDto } from "../bindings/JobDto";
import { notices } from "./notify.svelte";

type Listener = (job: JobDto) => void;

class Jobs {
  /** Running jobs, in start order. */
  running = $state<JobDto[]>([]);
  #finished = new Map<number, JobDto>();
  #waiters = new Map<number, ((job: JobDto) => void)[]>();
  #progressListeners = new Set<Listener>();

  constructor() {
    onJob((job) => this.#update(job));
  }

  #update(job: JobDto) {
    if (job.state === "running") {
      const i = this.running.findIndex((j) => j.id === job.id);
      if (i >= 0) this.running[i] = job;
      else this.running.push(job);
      this.#progressListeners.forEach((l) => l(job));
      return;
    }
    this.running = this.running.filter((j) => j.id !== job.id);
    this.#finished.set(job.id, job);
    if (job.state === "failed") notices.show(`${job.label} failed: ${job.error}`, "error");
    else if (job.state === "cancelled") notices.show(`${job.label} was cancelled`);
    // Sorting and indexing happen on their own; only their failures are news.
    else if (job.message && !["sort", "index", "filter"].includes(job.kind)) notices.show(job.message);
    this.#waiters.get(job.id)?.forEach((resolve) => resolve(job));
    this.#waiters.delete(job.id);
  }

  /** Resolves with the job's final status. */
  waitFor(id: number): Promise<JobDto> {
    const done = this.#finished.get(id);
    if (done) return Promise.resolve(done);
    return new Promise((resolve) => {
      this.#waiters.set(id, [...(this.#waiters.get(id) ?? []), resolve]);
    });
  }

  /** Calls `listener` on every progress update of any job. */
  onProgress(listener: Listener): () => void {
    this.#progressListeners.add(listener);
    return () => this.#progressListeners.delete(listener);
  }

  async cancel(id: number) {
    try {
      await api.cancelJob(id);
    } catch (e) {
      notices.error(e);
    }
  }
}

export const jobs = new Jobs();
