// Unit tests for what a window shows of the threat search (ENG-07): `npm
// test` in app/ runs them with Node's own test runner.
import assert from "node:assert/strict";
import { test } from "node:test";
import type { ThreatDto } from "./bindings/ThreatDto";
import {
  isCurrentThreat,
  isNewFailure,
  taskQueue,
  threatErrorNotice,
  visibleThreat,
} from "./threatView.ts";

const SCHOLAR = "r1bqkbnr/pppp1ppp/2n5/4p2Q/2B1P3/8/PPPP1PPP/RNB1K1NR b KQkq - 3 3";
const CHECK = "rnbqkbnr/ppppp1pp/8/5p1Q/4P3/8/PPPP1PPP/RNB1KBNR b KQkq - 1 2";

function threat(over: Partial<ThreatDto>): ThreatDto {
  return {
    seq: 1,
    session: 1,
    fen: SCHOLAR,
    depth: 0,
    finished: false,
    line: null,
    unavailable: null,
    error: null,
    startFailed: false,
    ...over,
  };
}

test("an update that was on its way when the board moved doesn't replace the note after it", () => {
  // The backend numbered the search's updates 4, then the note in check 5.
  const note = threat({ seq: 5, fen: CHECK, finished: true, unavailable: "inCheck" });
  const stale = threat({ seq: 4, depth: 12 });
  let newest = 0;
  let kept: ThreatDto | null = null;
  for (const t of [note, stale]) {
    if (!isCurrentThreat(t, newest)) continue;
    newest = t.seq;
    kept = t;
  }
  assert.equal(kept, note);
  assert.equal(visibleThreat(kept, { session: 1, fen: CHECK }, true)?.unavailable, "inCheck");
  // Updates of one search share its number: each one is taken.
  assert.ok(isCurrentThreat(threat({ seq: 5, depth: 13 }), 5));
});

test("a line or note shows only for the position it's about", () => {
  const t = threat({ seq: 2, line: null, depth: 3 });
  assert.equal(visibleThreat(t, { session: 1, fen: SCHOLAR }, true), t);
  assert.equal(visibleThreat(t, { session: 1, fen: CHECK }, true), null, "another position");
  assert.equal(
    visibleThreat(t, { session: 1, fen: SCHOLAR }, false),
    null,
    "off, or the engine stopped",
  );
  assert.equal(visibleThreat(t, null, true), null);
});

test("a failure stays in its game's window, whatever the board shows", () => {
  const failed = threat({ seq: 3, fen: "", error: "the engine exited (signal: 9 (SIGKILL))" });
  assert.equal(visibleThreat(failed, { session: 1, fen: CHECK }, true), failed, "after moves too");
  // Another window's game (another session) doesn't show it, nor notify.
  assert.equal(visibleThreat(failed, { session: 2, fen: SCHOLAR }, true), null);
  assert.equal(visibleThreat(failed, { session: 1, fen: SCHOLAR }, false), null);
});

test("a failure told again to the next game shows there but is announced once", () => {
  const error = "the engine exited (signal: 9 (SIGKILL))";
  const failed = threat({ seq: 3, session: 1, error });
  assert.ok(isNewFailure(null, failed));
  assert.ok(isNewFailure(threat({ seq: 2, depth: 9 }), failed));
  // A new game in this window: the analysis follows session 2, and so does the failure.
  const told = threat({ seq: 4, session: 2, error });
  assert.equal(isNewFailure(failed, told), false, "no second notice");
  assert.equal(visibleThreat(told, { session: 2, fen: CHECK }, true), told, "but the row shows it");
  assert.ok(isNewFailure(failed, threat({ seq: 5, session: 2, error, startFailed: true })));
  assert.equal(isNewFailure(failed, threat({ seq: 6, line: null })), false, "not a failure");
});

test("the notice says whether the threat engine stopped or couldn't start", () => {
  assert.equal(
    threatErrorNotice(threat({ error: "the engine exited (exit status: 3)", startFailed: true })),
    `Couldn't start the threat engine: the engine exited (exit status: 3). Turn "Show threat" off and on to retry.`,
  );
  assert.equal(
    threatErrorNotice(threat({ error: "the engine exited (signal: 9 (SIGKILL))" })),
    `The threat search stopped: the engine exited (signal: 9 (SIGKILL)). Turn "Show threat" off and on to retry.`,
  );
});

/** A backend whose "Show threat" call takes a moment (the engine starts). */
function slowBackend() {
  const b = { threat: false, calls: [] as boolean[], inFlight: 0, maxInFlight: 0 };
  const set = async (on: boolean) => {
    b.calls.push(on);
    b.inFlight++;
    b.maxInFlight = Math.max(b.maxInFlight, b.inFlight);
    await new Promise((r) => setTimeout(r, on ? 30 : 5));
    b.threat = on;
    b.inFlight--;
  };
  return { b, set };
}

test("a double click ends off: the second call waits for the first and sends the latest setting", async () => {
  const { b, set } = slowBackend();
  let showThreat = false;
  const queue = taskQueue();
  const send = () => queue(() => set(showThreat));
  showThreat = true;
  const first = send();
  // The first call is on its way (the engine starts) when the second click comes.
  await new Promise((r) => setTimeout(r, 5));
  assert.equal(b.inFlight, 1);
  showThreat = false;
  const second = send();
  await Promise.all([first, second]);
  assert.deepEqual(b.calls, [true, false]);
  assert.equal(b.threat, false);
  assert.equal(b.maxInFlight, 1, "never two at once");
});

test("on, off, on, off quickly ends off, one call at a time", async () => {
  const { b, set } = slowBackend();
  let showThreat = false;
  const queue = taskQueue();
  const send = () => queue(() => set(showThreat));
  const calls = [];
  for (const on of [true, false, true, false]) {
    showThreat = on;
    calls.push(send());
    await new Promise((r) => setTimeout(r, 2));
  }
  await Promise.all(calls);
  assert.equal(b.threat, false);
  assert.equal(b.calls.at(-1), false);
  assert.equal(b.maxInFlight, 1);
});

test("a call that fails doesn't stop the ones after it", async () => {
  let n = 0;
  const queue = taskQueue();
  const send = () =>
    queue(async () => {
      n++;
      if (n === 1) throw new Error("no engine");
    });
  await assert.rejects(send());
  await send();
  assert.equal(n, 2);
});

test("a status read queued after a call sees what the call did", async () => {
  // The threat engine came up (its first event) just as the user turned it
  // off: the read waits for the off, so the page doesn't end up saying on.
  const { b, set } = slowBackend();
  b.threat = true;
  const queue = taskQueue();
  let shown: boolean | null = null;
  const off = queue(() => set(false));
  const read = queue(async () => {
    shown = b.threat;
  });
  await Promise.all([off, read]);
  assert.equal(shown, false);
});
