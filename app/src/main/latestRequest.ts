/** A tiny generation gate for async work whose older results must be ignored. */
export class LatestRequest {
  private generation = 0;

  begin(): number {
    this.generation += 1;
    return this.generation;
  }

  invalidate(): void {
    this.generation += 1;
  }

  /** Текущее поколение без начала нового: для проверок, которые сами ничего не отменяют. */
  token(): number {
    return this.generation;
  }

  isCurrent(token: number): boolean {
    return token === this.generation;
  }
}

export type TranslationChange = "edit" | "target" | "prefill" | "swap" | "unmount";

type Schedule = (callback: () => void, delay: number) => unknown;
type Cancel = (handle: unknown) => void;

/** Owns both the debounce and request generation so a change cancels them atomically. */
export class TranslationController {
  private readonly requests = new LatestRequest();
  private readonly scheduleTimer: Schedule;
  private readonly cancelTimer: Cancel;
  private pending: unknown | undefined;

  constructor(
    scheduleTimer: Schedule = (callback, delay) => window.setTimeout(callback, delay),
    cancelTimer: Cancel = (handle) => window.clearTimeout(handle as number),
  ) {
    this.scheduleTimer = scheduleTimer;
    this.cancelTimer = cancelTimer;
  }

  schedule(callback: () => void, delay: number): void {
    this.clearPending();
    this.pending = this.scheduleTimer(() => {
      this.pending = undefined;
      callback();
    }, delay);
  }

  clearPending(): void {
    if (this.pending === undefined) return;
    this.cancelTimer(this.pending);
    this.pending = undefined;
  }

  change(_reason: TranslationChange): void {
    this.clearPending();
    this.requests.invalidate();
  }

  begin(): number {
    this.clearPending();
    return this.requests.begin();
  }

  isCurrent(token: number): boolean {
    return this.requests.isCurrent(token);
  }
}

/** Runs mutations one by one, preserving the user's click order in SQLite. */
export class AsyncQueue {
  private tail: Promise<void> = Promise.resolve();

  enqueue<T>(task: () => Promise<T>): Promise<T> {
    const result = this.tail.then(task, task);
    this.tail = result.then(() => undefined, () => undefined);
    return result;
  }
}

/** Serializes favorite writes and remembers only DB-confirmed state for the visible entry. */
export class FavoriteController {
  private readonly queue = new AsyncQueue();
  private readonly requests = new LatestRequest();
  private current: { historyId: number; confirmed: boolean } | null = null;

  accept(historyId: number | null, favorite: boolean): void {
    this.requests.invalidate();
    this.current = historyId === null ? null : { historyId, confirmed: favorite };
  }

  clear(): void {
    this.requests.invalidate();
    this.current = null;
  }

  async mutate(historyId: number, favorite: boolean, write: () => Promise<void>): Promise<boolean | null> {
    const token = this.requests.begin();
    try {
      await this.queue.enqueue(write);
      if (this.current?.historyId === historyId) this.current.confirmed = favorite;
      return null;
    } catch {
      if (!this.requests.isCurrent(token) || this.current?.historyId !== historyId) return null;
      return this.current.confirmed;
    }
  }
}
