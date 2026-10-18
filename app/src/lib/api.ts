// Typed wrappers around the Tauri commands in app/src-tauri/src/commands.
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { AnalysisDto } from "./bindings/AnalysisDto";
import type { AnalysisStatusDto } from "./bindings/AnalysisStatusDto";
import type { AppError } from "./bindings/AppError";
import type { BoardImageDto } from "./bindings/BoardImageDto";
import type { CommentSlotDto } from "./bindings/CommentSlotDto";
import type { CopyModeDto } from "./bindings/CopyModeDto";
import type { CrossTableDto } from "./bindings/CrossTableDto";
import type { DbInfoDto } from "./bindings/DbInfoDto";
import type { DbViewDto } from "./bindings/DbViewDto";
import type { EngineDto } from "./bindings/EngineDto";
import type { FenCheckDto } from "./bindings/FenCheckDto";
import type { GameLineDto } from "./bindings/GameLineDto";
import type { GamePage } from "./bindings/GamePage";
import type { GotoPositionDto } from "./bindings/GotoPositionDto";
import type { GameViewDto } from "./bindings/GameViewDto";
import type { HeaderQueryDto } from "./bindings/HeaderQueryDto";
import type { HeadersDto } from "./bindings/HeadersDto";
import type { ImageFormatDto } from "./bindings/ImageFormatDto";
import type { ImportReportDto } from "./bindings/ImportReportDto";
import type { JobDto } from "./bindings/JobDto";
import type { LegalMoveDto } from "./bindings/LegalMoveDto";
import type { ListSelectionDto } from "./bindings/ListSelectionDto";
import type { NameKindDto } from "./bindings/NameKindDto";
import type { NameListDto } from "./bindings/NameListDto";
import type { OnboardingDto } from "./bindings/OnboardingDto";
import type { PastedGameDto } from "./bindings/PastedGameDto";
import type { PlayResultDto } from "./bindings/PlayResultDto";
import type { RecentDbDto } from "./bindings/RecentDbDto";
import type { ReferenceDbDto } from "./bindings/ReferenceDbDto";
import type { ReferenceDto } from "./bindings/ReferenceDto";
import type { ReferenceLinesDto } from "./bindings/ReferenceLinesDto";
import type { SaveModeDto } from "./bindings/SaveModeDto";
import type { ShapeDto } from "./bindings/ShapeDto";
import type { SortSpecDto } from "./bindings/SortSpecDto";
import type { StepDto } from "./bindings/StepDto";
import type { ThreatDto } from "./bindings/ThreatDto";
import type { TreeDto } from "./bindings/TreeDto";
import type { UiPrefChangeDto } from "./bindings/UiPrefChangeDto";
import type { UiPrefsDto } from "./bindings/UiPrefsDto";
import type { VariationPolicyDto } from "./bindings/VariationPolicyDto";

export const api = {
  createDb: (path: string) => invoke<DbInfoDto>("create_db", { path }),
  openDb: (path: string) => invoke<DbInfoDto>("open_db", { path }),
  peekDatabase: (path: string, limit: number) => invoke<GamePage>("peek_database", { path, limit }),
  peekGameLine: (path: string, game: number) => invoke<GameLineDto>("peek_game_line", { path, game }),
  openEditorWindow: (path: string | null) => invoke<void>("open_editor_window", { path }),
  takeEditorTarget: (label: string) => invoke<string | null>("take_editor_target", { label }),
  listWindows: () => invoke<string[]>("list_windows"),
  closeDb: (handle: number) => invoke<void>("close_db", { handle }),
  referenceDb: () => invoke<ReferenceDbDto | null>("reference_db"),
  setReferenceDb: (path: string | null) => invoke<ReferenceDbDto | null>("set_reference_db", { path }),
  dbInfo: (handle: number) => invoke<DbInfoDto>("db_info", { handle }),
  listGames: (handle: number, sort: SortSpecDto, offset: number, limit: number) =>
    invoke<GamePage>("list_games", { handle, sort, offset, limit }),
  sortGames: (handle: number, sort: SortSpecDto) =>
    invoke<number | null>("sort_games", { handle, sort }),
  importPgn: (handle: number, path: string) => invoke<number>("import_pgn", { handle, path }),
  /** Imports a ChessBase database (IO-03), given its `.cbh` file. */
  importCb: (handle: number, path: string) => invoke<number>("import_cb", { handle, path }),
  importReport: (handle: number) => invoke<ImportReportDto | null>("import_report", { handle }),
  exportPgn: (handle: number, path: string, ids: number[] | null = null) =>
    invoke<number>("export_pgn", { handle, path, ids }),
  setDeleted: (handle: number, ids: number[], deleted: boolean) =>
    invoke<void>("set_deleted", { handle, ids, deleted }),
  setDeletedRows: (handle: number, selection: ListSelectionDto, deleted: boolean) =>
    invoke<number>("set_deleted_rows", { handle, selection, deleted }),
  copyGames: (handle: number, selection: ListSelectionDto, target: string, create: boolean, mode: CopyModeDto) =>
    invoke<number>("copy_games", { handle, selection, target, create, mode }),
  searchGames: (handle: number, query: Partial<HeaderQueryDto>, within: boolean, quick: boolean) =>
    invoke<number | null>("search_games", { handle, query, within, quick }),
  clearFilter: (handle: number) => invoke<DbInfoDto>("clear_filter", { handle }),
  compactDb: (handle: number) => invoke<number>("compact_db", { handle }),
  dbView: (handle: number) => invoke<DbViewDto>("db_view", { handle }),
  saveDbLayout: (handle: number, sort: SortSpecDto, columns: Record<string, number>) =>
    invoke<void>("save_db_layout", { handle, sort, columns }),
  restoreFilter: (handle: number) => invoke<number | null>("restore_filter", { handle }),
  listNames: (handle: number, kind: NameKindDto, text: string, byGames: boolean, limit: number) =>
    invoke<NameListDto>("list_names", { handle, kind, text, byGames, limit }),
  showNameGames: (handle: number, kind: NameKindDto, id: number) =>
    invoke<DbInfoDto>("show_name_games", { handle, kind, id }),
  renameName: (handle: number, kind: NameKindDto, id: number, name: string) =>
    invoke<string>("rename_name", { handle, kind, id, name }),
  /** A tournament cross-table (DB-16): of event `event`, or of `game`'s event. */
  crossTable: (handle: number, event: number | null, game: number | null, edition: number | null = null) =>
    invoke<CrossTableDto>("cross_table", { handle, event, game, edition }),
  /** Limits the list to one edition of an event (DB-16's Show games). */
  showEditionGames: (handle: number, event: number, edition: number) =>
    invoke<DbInfoDto>("show_edition_games", { handle, event, edition }),
  updateIndex: (handle: number) => invoke<number | null>("update_index", { handle }),
  openingTree: (handle: number, session: number) => invoke<TreeDto>("opening_tree", { handle, session }),
  searchPosition: (handle: number, session: number) =>
    invoke<DbInfoDto>("search_position", { handle, session }),
  scanPositionGames: (handle: number, session: number) =>
    invoke<number>("scan_position_games", { handle, session }),
  referenceView: (handle: number, session: number) =>
    invoke<ReferenceDto>("reference_view", { handle, session }),
  referenceLines: (handle: number, session: number) =>
    invoke<ReferenceLinesDto>("reference_lines", { handle, session }),
  gameLine: (handle: number, game: number, session: number) =>
    invoke<GameLineDto>("game_line", { handle, game, session }),

  openGame: (handle: number, game: number, ply: number | null = null) =>
    invoke<GameViewDto>("open_game", { handle, game, ply }),
  newGame: (fen: string | null = null) => invoke<GameViewDto>("new_game", { fen }),
  closeSession: (session: number) => invoke<void>("close_session", { session }),
  checkFen: (fen: string) => invoke<FenCheckDto>("check_fen", { fen }),
  gotoPosition: (session: number | null, text: string) =>
    invoke<GotoPositionDto>("goto_position", { session, text }),
  activateSession: (session: number) => invoke<GameViewDto>("activate_session", { session }),
  gameView: (session: number) => invoke<GameViewDto>("game_view_cmd", { session }),
  gotoNode: (session: number, node: number) => invoke<GameViewDto>("goto_node", { session, node }),
  step: (session: number, dir: StepDto) => invoke<GameViewDto>("step", { session, dir }),
  playMove: (
    session: number,
    orig: string,
    dest: string,
    promotion: string | null = null,
    policy: VariationPolicyDto | null = null,
  ) => invoke<PlayResultDto>("play_move", { session, orig, dest, promotion, policy }),
  legalMoves: (session: number) => invoke<LegalMoveDto[]>("legal_moves", { session }),
  promoteVariation: (session: number, node: number) =>
    invoke<GameViewDto>("promote_variation", { session, node }),
  deleteVariation: (session: number, node: number) =>
    invoke<GameViewDto>("delete_variation", { session, node }),
  deleteRemaining: (session: number, node: number) =>
    invoke<GameViewDto>("delete_remaining", { session, node }),
  setComment: (session: number, node: number, slot: CommentSlotDto, text: string) =>
    invoke<GameViewDto>("set_comment", { session, node, slot, text }),
  toggleNag: (session: number, node: number, nag: number) =>
    invoke<GameViewDto>("toggle_nag", { session, node, nag }),
  setShapes: (session: number, shapes: ShapeDto[]) =>
    invoke<GameViewDto>("set_shapes", { session, shapes }),
  setHeaders: (session: number, headers: HeadersDto) =>
    invoke<GameViewDto>("set_headers", { session, headers }),
  undo: (session: number) => invoke<GameViewDto>("undo", { session }),
  redo: (session: number) => invoke<GameViewDto>("redo", { session }),
  saveGame: (session: number, mode: SaveModeDto, handle: number | null = null) =>
    invoke<GameViewDto>("save_game", { session, mode, handle }),
  gamePgn: (session: number) => invoke<string>("game_pgn", { session }),
  pasteGame: (session: number | null, text: string) =>
    invoke<PastedGameDto>("paste_game", { session, text }),
  clipboardText: () => invoke<string | null>("clipboard_text"),
  copyText: (text: string) => invoke<void>("copy_text", { text }),
  copyBoardImage: (session: number, image: BoardImageDto, format: ImageFormatDto) =>
    invoke<void>("copy_board_image", { session, image, format }),
  saveBoardImage: (session: number, image: BoardImageDto, path: string) =>
    invoke<void>("save_board_image", { session, image, path }),

  cancelJob: (id: number) => invoke<boolean>("cancel_job", { id }),
  listJobs: () => invoke<JobDto[]>("list_jobs"),

  listEngines: () => invoke<EngineDto[]>("list_engines"),
  detectEngines: () => invoke<EngineDto[]>("detect_engines"),
  addEngine: (path: string) => invoke<EngineDto[]>("add_engine", { path }),
  removeEngine: (id: string) => invoke<EngineDto[]>("remove_engine", { id }),
  setDefaultEngine: (id: string) => invoke<EngineDto[]>("set_default_engine", { id }),
  setEngineOptions: (id: string, options: [string, string][]) =>
    invoke<EngineDto[]>("set_engine_options", { id, options }),
  startAnalysis: (session: number, engine: string | null, multipv: number, threat = false) =>
    invoke<AnalysisStatusDto>("start_analysis", { session, engine, multipv, threat }),
  stopAnalysis: () => invoke<AnalysisStatusDto>("stop_analysis"),
  analysisStatus: () => invoke<AnalysisStatusDto>("analysis_status"),
  setAnalysisLines: (multipv: number) => invoke<AnalysisStatusDto>("set_analysis_lines", { multipv }),
  /** Turns the threat search (ENG-07) of the running analysis on or off. */
  setAnalysisThreat: (on: boolean) => invoke<AnalysisStatusDto>("set_analysis_threat", { on }),
  addEngineLine: (session: number, moves: string[], comment: string | null, goto: boolean) =>
    invoke<GameViewDto>("add_engine_line", { session, moves, comment, goto }),
  blunderCheck: (session: number, engine: string | null, movetimeMs: number) =>
    invoke<number>("blunder_check", { session, engine, movetimeMs }),

  recentDatabases: () => invoke<RecentDbDto[]>("recent_databases"),
  forgetRecentDatabase: (path: string) => invoke<void>("forget_recent_database", { path }),
  uiPrefs: () => invoke<UiPrefsDto>("ui_prefs"),
  /** Saves one preference (null removes it); resolves to the revision it's saved at. */
  setUiPref: (key: string, value: unknown) => invoke<number>("set_ui_pref", { key, value }),

  onboardingStatus: () => invoke<OnboardingDto>("onboarding_status"),
  finishWelcome: () => invoke<void>("finish_welcome"),
  openSampleDb: () => invoke<DbInfoDto>("open_sample_db"),
  suggestDbPath: (name: string) => invoke<string>("suggest_db_path", { name }),
};

export function onAnalysis(cb: (update: AnalysisDto) => void): Promise<UnlistenFn> {
  return listen<AnalysisDto>("analysis", (e) => cb(e.payload));
}

/** The opponent's threat in the analysed position (ENG-07). */
export function onThreat(cb: (threat: ThreatDto) => void): Promise<UnlistenFn> {
  return listen<ThreatDto>("threat", (e) => cb(e.payload));
}

export function onJob(cb: (job: JobDto) => void): Promise<UnlistenFn> {
  return listen<JobDto>("job", (e) => cb(e.payload));
}

/** A preference saved by any window, this one included (DB-07). */
export function onUiPref(cb: (change: UiPrefChangeDto) => void): Promise<UnlistenFn> {
  return listen<UiPrefChangeDto>("ui-pref", (e) => cb(e.payload));
}

export function isAppError(e: unknown): e is AppError {
  return typeof e === "object" && e !== null && "code" in e && "message" in e;
}

export function errorMessage(e: unknown): string {
  return isAppError(e) ? e.message : String(e);
}
