import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { mkdtemp, mkdir, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { createInterface } from "node:readline";
import { setTimeout as delay } from "node:timers/promises";

const binary = path.resolve(process.argv[2] ?? "target/debug/ahead");
const supportedAdapters = ["pi-acp", "codex-acp", "claude-acp"];
const requestedAdapters = process.argv.slice(3);
const adapterIds = requestedAdapters.length ? requestedAdapters : supportedAdapters;
const livePiTurn = process.env.AHEAD_ACP_LIVE_PI_TURN === "1";
const apiKey = process.env.OPENROUTER_API_KEY;
const piModel = process.env.AHEAD_PI_TEST_MODEL
  ?? "openrouter/nvidia/nemotron-3-ultra-550b-a55b:free";
for (const adapterId of adapterIds) {
  assert(supportedAdapters.includes(adapterId), `Unsupported curated adapter: ${adapterId}`);
}
if (livePiTurn) {
  assert.deepEqual(adapterIds, ["pi-acp"], "The authenticated turn is scoped to Pi only");
  assert(apiKey, "Set OPENROUTER_API_KEY for the authenticated Pi ACP turn");
}

function sanitize(value) {
  return apiKey ? String(value).replaceAll(apiKey, "[REDACTED]") : String(value);
}

const smokeRoot = await mkdtemp(path.join(tmpdir(), "ahead-acp-install-"));
let workspace;
let environment;
let child;
let lines;
let stderr = "";
let exited = false;
const pending = new Map();
const notifications = [];
let nextId = 0;

async function createProfile(adapterId) {
  const profile = path.join(smokeRoot, adapterId);
  workspace = path.join(profile, "project");
  environment = {
    PATH: process.env.PATH ?? "/usr/bin:/bin:/usr/sbin:/sbin",
    HOME: path.join(profile, "home"),
    TMPDIR: profile,
    LANG: process.env.LANG,
    LC_ALL: process.env.LC_ALL,
    RUST_LOG: "warn",
    AHEAD_DATA_HOME: path.join(profile, "data"),
    AHEAD_HOME: path.join(profile, "runtime"),
    AHEAD_USER_HOME: path.join(profile, "user"),
  };
  if (livePiTurn) {
    environment.OPENROUTER_API_KEY = apiKey;
    environment.PI_CODING_AGENT_DIR = path.join(environment.HOME, ".pi", "agent");
  }
  for (const directory of [
    workspace,
    environment.HOME,
    environment.AHEAD_DATA_HOME,
    environment.AHEAD_HOME,
    environment.AHEAD_USER_HOME,
  ]) {
    await mkdir(directory, { recursive: true });
  }
  await mkdir(path.join(workspace, ".ahead"));
  if (livePiTurn) {
    await mkdir(environment.PI_CODING_AGENT_DIR, { recursive: true });
    const modelId = piModel.replace(/^openrouter\//, "");
    await writeFile(
      path.join(environment.PI_CODING_AGENT_DIR, "models.json"),
      JSON.stringify({
        providers: {
          openrouter: {
            baseUrl: "https://openrouter.ai/api/v1",
            api: "openai-completions",
            models: [{ id: modelId, name: "AHEAD Pi ACP smoke model" }],
          },
        },
      }),
      { mode: 0o600 },
    );
  }
  return profile;
}

function resetProxyState() {
  child = undefined;
  lines = undefined;
  stderr = "";
  exited = false;
  pending.clear();
  notifications.length = 0;
  nextId = 0;
}

function rpc(method, params, timeoutMs = 5 * 60 * 1000) {
  const id = ++nextId;
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => {
      pending.delete(id);
      reject(new Error(`Request timed out: ${method}\n${stderr}`));
    }, timeoutMs);
    pending.set(id, { resolve, reject, timer });
    const request = Object.keys(params).length === 0
      ? { method }
      : { method, params };
    child.stdin.write(JSON.stringify({
      id,
      method: "ahead_request",
      params: { request },
    }) + "\n");
  });
}

function startProxy() {
  child = spawn(binary, ["--proxy"], {
    cwd: workspace,
    detached: true,
    stdio: ["pipe", "pipe", "pipe"],
    env: environment,
  });
  child.stderr.on("data", (chunk) => {
    stderr = sanitize(stderr + chunk).slice(-8000);
  });
  child.once("exit", () => {
    exited = true;
    for (const waiter of pending.values()) {
      clearTimeout(waiter.timer);
      waiter.reject(new Error(`Proxy exited before its reply\n${stderr}`));
    }
    pending.clear();
  });
  lines = createInterface({ input: child.stdout });
  lines.on("line", (line) => {
    let message;
    try {
      message = JSON.parse(line);
    } catch (error) {
      stderr = `${stderr}\nInvalid proxy output: ${error}`.slice(-8000);
      return;
    }
    if (message.method === "ahead_notification") {
      notifications.push(message.params?.notification);
      return;
    }
    const waiter = pending.get(message.id);
    if (!waiter || message.method) return;
    pending.delete(message.id);
    clearTimeout(waiter.timer);
    if (message.error) waiter.reject(new Error(sanitize(JSON.stringify(message.error))));
    else waiter.resolve(message.result.params.response);
  });
  child.stdin.write(JSON.stringify({
    method: "initialize",
    params: { workspace, window_id: 1, tab_id: 1 },
  }) + "\n");
}

async function waitUntil(predicate, description, timeoutMs = 15000) {
  const deadline = Date.now() + timeoutMs;
  while (!await predicate()) {
    assert(Date.now() < deadline, `Timed out: ${description}\n${stderr}`);
    await delay(30);
  }
}

async function stopProxy() {
  if (!child) return;
  if (!exited) {
    child.stdin.write(JSON.stringify({ method: "shutdown", params: {} }) + "\n");
    try {
      await waitUntil(() => exited, "proxy shutdown", 10000);
    } catch {
      process.kill(-child.pid, "SIGKILL");
      await waitUntil(() => exited, "proxy process-group teardown", 5000);
    }
  }
  lines?.close();
  child = undefined;
  lines = undefined;
}

async function removeSmokePath(targetPath) {
  // Some ACP servers finish writing profile-local logs just after the proxy
  // exits; let those writes settle before removing the disposable profile.
  const deadline = Date.now() + 30000;
  while (true) {
    try {
      await rm(targetPath, { recursive: true, force: true });
      return;
    } catch (error) {
      if (!['EBUSY', 'ENOTEMPTY', 'EPERM'].includes(error.code) || Date.now() >= deadline) {
        throw error;
      }
      await delay(100);
    }
  }
}

try {
  console.log(`Starting isolated ACP install smoke for ${adapterIds.join(", ")}`);
  for (const adapterId of adapterIds) {
    const profile = await createProfile(adapterId);
    resetProxyState();
    startProxy();
    try {
      console.log(`${adapterId}: loading curated ACP catalog`);
      const catalog = await rpc("list_external_acp_adapters", {});
      assert.deepEqual(
        catalog.map((adapter) => adapter.id),
        supportedAdapters,
        "AHEAD must expose only its three supported ACP adapters",
      );
      const catalogEntry = catalog.find((adapter) => adapter.id === adapterId);
      assert(catalogEntry.available, `${adapterId}: no registry or pinned distribution available`);
      console.log(`Installing ${adapterId} (${catalogEntry.latest_version ?? "registry fallback"})`);
      await rpc("set_external_acp_adapter_installed", {
        adapter_id: adapterId,
        installed: true,
      });

      const view = await rpc("start_work", {
        work_kind: "product-change",
        title: `ACP install smoke: ${adapterId}`,
        starting_point: "Initialize this installed ACP agent without sending a prompt.",
        work_item: null,
        harness: "external_acp",
        external_agent_id: adapterId,
        parent_session_id: null,
      });

      let prepareError;
      try {
        console.log(`Preparing ${adapterId} ACP session`);
        await rpc("agent_session_prepare", { session_id: view.session.id });
      } catch (error) {
        prepareError = String(error);
        const requestedAuthentication = notifications.some((notification) =>
          notification?.method === "agent_authentication_required"
          && notification.params?.session_id === view.session.id,
        );
        assert(
          requestedAuthentication || /authentication required|sign in required/i.test(prepareError),
          `${adapterId}: ACP session preparation failed before authentication: ${prepareError}`,
        );
      }

      const refreshedCatalog = await rpc("list_external_acp_adapters", {});
      const installed = refreshedCatalog.find((adapter) => adapter.id === adapterId);
      assert(installed?.installed, `${adapterId}: installed state was not persisted`);
      assert(installed.installed_version, `${adapterId}: no installed package provenance was recorded`);

      await waitUntil(
        () => notifications.some((notification) =>
          notification?.params?.session_id === view.session.id
          && ["agent_config_options_available", "agent_authentication_required"]
            .includes(notification.method),
        ),
        `${adapterId} config options or authentication state`,
      );
      const options = notifications
        .filter((notification) => notification?.method === "agent_config_options_available"
          && notification.params?.session_id === view.session.id)
        .at(-1)?.params?.options ?? [];
      const hasModelOption = options.some((option) => /model/i.test(`${option.id} ${option.name}`));
      const needsAuthentication = notifications.some((notification) =>
        notification?.method === "agent_authentication_required"
        && notification.params?.session_id === view.session.id,
      );
      assert(
        hasModelOption || needsAuthentication,
        `${adapterId}: neither model options nor authentication state was advertised`,
      );

      const selectableModel = options.find((option) =>
        /model/i.test(`${option.id} ${option.name}`)
        && option.current_value?.type === "select"
        && Array.isArray(option.choices)
        && option.choices.some((choice) => choice.value !== option.current_value.value),
      );
      let selectedModelAfterUpdate;
      if (selectableModel) {
        const choice = selectableModel.choices.find(
          (candidate) => candidate.value !== selectableModel.current_value.value,
        );
        const response = await rpc("agent_config_option_set", {
          session_id: view.session.id,
          config_id: selectableModel.id,
          value: { type: "select", value: choice.value },
        });
        assert.equal(response.updated, true, `${adapterId}: model selection was rejected`);
        await waitUntil(
          () => notifications.some((notification) =>
            notification?.method === "agent_config_options_available"
            && notification.params?.session_id === view.session.id
            && notification.params?.options?.some((option) =>
              option.id === selectableModel.id
              && option.current_value?.value === choice.value,
            ),
          ),
          `${adapterId} live model-option update`,
        );
        console.log(`  Selected alternate model: ${choice.name ?? choice.value}`);
        selectedModelAfterUpdate = { id: selectableModel.id, value: choice.value };
      }
      if (adapterId === "claude-acp") {
        assert(selectableModel, "Claude ACP must expose a selectable model and accept a live update");
      }
      if (livePiTurn) {
        const modelOption = options.find((option) =>
          option.category === "model"
          && option.current_value?.type === "select"
          && option.choices?.some((choice) => choice.value === piModel),
        );
        assert(modelOption, `Pi did not expose ${piModel} in its model options`);
        if (modelOption.current_value.value !== piModel) {
          const response = await rpc("agent_config_option_set", {
            session_id: view.session.id,
            config_id: modelOption.id,
            value: { type: "select", value: piModel },
          });
          assert.equal(response.updated, true, "Pi model selection was rejected");
          await waitUntil(
            () => notifications.some((notification) =>
              notification?.method === "agent_config_options_available"
              && notification.params?.session_id === view.session.id
              && notification.params?.options?.some((option) =>
                option.id === modelOption.id
                && option.current_value?.value === piModel,
              ),
            ),
            "Pi OpenRouter model selection",
          );
        }
        selectedModelAfterUpdate = { id: modelOption.id, value: piModel };

        const { turn_id: turnId } = await rpc("agent_turn_start", {
          request: {
            session_id: view.session.id,
            thread_id: `thread-${view.session.id}`,
            harness: "external_acp",
            external_agent_id: "pi-acp",
            model: piModel,
            model_provider: null,
            model_context_window: null,
            reasoning_effort: null,
            user_message: "Reply exactly AHEAD_PI_ACP_PONG. Do not use tools.",
            session_context: "Disposable authenticated Pi ACP smoke.",
            context: {
              active_path: "",
              caret: { line: 0, col: 0 },
              selection: null,
              file_content: "",
              visible_end: null,
              attached_anchor_ids: [],
              attached_files: [],
              attached_memories: [],
            },
            invariants: [],
            cwd: workspace,
            expected_policy_sha256: view.session.policy.sha256,
            read_only: false,
            scope: null,
          },
        });
        let history;
        await waitUntil(async () => {
          history = await rpc("conversation_messages_page", {
            session_id: view.session.id,
            before: null,
            limit: 12,
          });
          return history.messages.some((message) =>
            message.turn_id === turnId
            && message.role === "agent"
            && ["complete", "failed", "cancelled"].includes(message.status),
          );
        }, "authenticated Pi ACP turn", 180000);
        const agentMessage = history.messages.find((message) =>
          message.turn_id === turnId && message.role === "agent",
        );
        assert.equal(
          agentMessage.status,
          "complete",
          sanitize(JSON.stringify({ agentMessage, stderr })),
        );
        assert(agentMessage.content.includes("AHEAD_PI_ACP_PONG"),
          "Pi ACP did not return the requested response");
      }

      await stopProxy();
      resetProxyState();
      startProxy();
      let reconnectError;
      try {
        await rpc("agent_session_prepare", { session_id: view.session.id });
      } catch (error) {
        reconnectError = String(error);
      }
      await waitUntil(
        () => Boolean(reconnectError) || notifications.some((notification) =>
          notification?.params?.session_id === view.session.id
          && ["agent_config_options_available", "agent_authentication_required"]
            .includes(notification.method),
        ),
        `${adapterId} session reconnect`,
      );
      const reconnectedOptions = notifications
        .filter((notification) => notification?.method === "agent_config_options_available"
          && notification.params?.session_id === view.session.id)
        .at(-1)?.params?.options ?? [];
      const reconnectedAuthenticationRequired = notifications.some((notification) =>
        notification?.method === "agent_authentication_required"
        && notification.params?.session_id === view.session.id,
      );
      assert(
        reconnectedOptions.length > 0 || reconnectedAuthenticationRequired,
        `${adapterId}: session did not reconnect${reconnectError ? `: ${reconnectError}` : ""}`,
      );
      if (selectedModelAfterUpdate) {
        assert(
          reconnectedOptions.some((option) =>
            option.id === selectedModelAfterUpdate.id
            && option.current_value?.value === selectedModelAfterUpdate.value,
          ),
          `${adapterId}: selected model was not restored after reconnect`,
        );
      }
      console.log(`  Prepared ${adapterId} session after proxy restart`);

      await rpc("set_external_acp_adapter_installed", {
        adapter_id: adapterId,
        installed: false,
      });
      const catalogAfterRemoval = await rpc("list_external_acp_adapters", {});
      assert.equal(
        catalogAfterRemoval.find((adapter) => adapter.id === adapterId)?.installed,
        false,
        `${adapterId}: removal did not clear the installed catalog state`,
      );
      console.log(`  Removed ${adapterId} from AHEAD's installed catalog`);

      console.log(
        `PASS ${adapterId} ${installed.installed_version}: ACP launch, `
        + `${hasModelOption ? `${options.length} config options` : "authentication required"}`
        + `${livePiTurn ? ", authenticated model turn" : ""}`,
      );
    } finally {
      await stopProxy();
      await removeSmokePath(profile);
    }
  }
} catch (error) {
  console.error(sanitize(error?.stack ?? error));
  process.exitCode = 1;
} finally {
  if (!exited) await stopProxy().catch((error) => console.error(error));
  await removeSmokePath(smokeRoot);
}
