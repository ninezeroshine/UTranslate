import assert from "node:assert/strict";
import test from "node:test";

import { FavoriteController, TranslationController, type TranslationChange } from "./latestRequest.ts";

const deferred = <T>() => {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => { resolve = done; });
  return { promise, resolve };
};

test("an older translation cannot commit after a newer one", async () => {
  const controller = new TranslationController();
  const first = deferred<string>();
  const second = deferred<string>();
  const committed: string[] = [];

  const firstToken = controller.begin();
  const firstRun = first.promise.then((value) => {
    if (controller.isCurrent(firstToken)) committed.push(value);
  });

  const secondToken = controller.begin();
  const secondRun = second.promise.then((value) => {
    if (controller.isCurrent(secondToken)) committed.push(value);
  });

  second.resolve("new");
  await secondRun;
  first.resolve("stale");
  await firstRun;

  assert.deepEqual(committed, ["new"]);
});

test("editing invalidates an in-flight translation before the debounce starts another", async () => {
  const controller = new TranslationController();
  const response = deferred<string>();
  const committed: string[] = [];

  const token = controller.begin();
  const run = response.promise.then((value) => {
    if (controller.isCurrent(token)) committed.push(value);
  });

  controller.change("edit");
  response.resolve("stale");
  await run;

  assert.deepEqual(committed, []);
});

test("every source-changing action cancels the old debounce before invalidating its request", () => {
  const actions: TranslationChange[] = ["edit", "target", "prefill", "swap", "unmount"];

  for (const action of actions) {
    const timers = new Map<number, () => void>();
    let nextTimer = 0;
    let starts = 0;
    const controller = new TranslationController(
      (callback) => { const id = ++nextTimer; timers.set(id, callback); return id; },
      (handle) => timers.delete(handle as number),
    );

    controller.schedule(() => { starts += 1; controller.begin(); }, 600);
    controller.change(action);
    for (const callback of timers.values()) callback();

    assert.equal(starts, 0, `${action} left a stale debounce alive`);
  }
});

test("favorite writes are serialized and two failures roll back to the confirmed DB state", async () => {
  const favorites = new FavoriteController();
  favorites.accept(7, false);
  const first = deferred<void>();
  const second = deferred<void>();
  const events: string[] = [];

  const a = favorites.mutate(7, true, async () => {
    events.push("first:start");
    await first.promise;
    events.push("first:fail");
    throw new Error("first failed");
  });
  const b = favorites.mutate(7, false, async () => {
    events.push("second:start");
    await second.promise;
    events.push("second:fail");
    throw new Error("second failed");
  });

  await Promise.resolve();
  second.resolve();
  await Promise.resolve();
  assert.deepEqual(events, ["first:start"]);

  first.resolve();
  assert.equal(await a, null, "the stale first failure must not roll back the latest click");
  assert.equal(await b, false, "the latest failure must restore the last DB-confirmed value");
  assert.deepEqual(events, ["first:start", "first:fail", "second:start", "second:fail"]);
});

test("favorite rollback follows the last successful queued write", async () => {
  const favorites = new FavoriteController();
  favorites.accept(7, false);

  const first = favorites.mutate(7, true, async () => undefined);
  const second = favorites.mutate(7, false, async () => { throw new Error("second failed"); });

  assert.equal(await first, null);
  assert.equal(await second, true);
});

test("an old entry cannot replace the confirmed state of a newer result", async () => {
  const favorites = new FavoriteController();
  const oldWrite = deferred<void>();
  favorites.accept(7, false);
  const old = favorites.mutate(7, true, () => oldWrite.promise);

  favorites.accept(8, true);
  oldWrite.resolve();
  assert.equal(await old, null);

  const currentFailure = await favorites.mutate(8, false, async () => { throw new Error("current failed"); });
  assert.equal(currentFailure, true);
});
