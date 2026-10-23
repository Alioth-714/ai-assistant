(function () {
    "use strict";

    var roles = Array.isArray(window.ROLES) ? window.ROLES : [];
    var tauri = window.__TAURI__ || null;
    var invokeFn = null;
    if (tauri) {
        if (typeof tauri.invoke === "function") {
            invokeFn = tauri.invoke.bind(tauri);
        } else if (tauri.core && typeof tauri.core.invoke === "function") {
            invokeFn = tauri.core.invoke.bind(tauri.core);
        }
    }

    var histories = {};
    var currentRole = roles.length ? roles[0].id : "dev";
    var mode = "mock";
    var realReady = false;
    var sending = false;
    var agentCount = 0;
    var pendingConfirm = 0;
    var routing = false;
    var lastRoute = null;
    var routeSeq = 0;
    var historyLoaded = {};
    var permissionQueue = [];
    var activePermission = null;
    var activeTaskCard = null;
    var activeTaskId = null;
    var taskRole = null;
    var envDismissed = false;
    var els = {};

    function $(id) {
        return document.getElementById(id);
    }

    function roleById(id) {
        for (var i = 0; i < roles.length; i += 1) {
            if (roles[i].id === id) {
                return roles[i];
            }
        }
        return null;
    }

    function roleName(id) {
        var role = roleById(id);
        return role ? role.name : id;
    }

    function timeText() {
        return new Date().toLocaleTimeString("zh-CN", { hour12: false });
    }

    function sleep(ms) {
        return new Promise(function (resolve) {
            window.setTimeout(resolve, ms);
        });
    }

    function formatConfidence(value) {
        var number = Number(value);
        return Number.isFinite(number) ? number.toFixed(2) : "0.00";
    }

    function scrollToBottom() {
        els.history.scrollTop = els.history.scrollHeight;
    }

    function toolSummary(tools) {
        if (!Array.isArray(tools) || !tools.length) {
            return "";
        }
        return " · 工具: " + tools.map(function (tool) {
            return (tool.name || "-") + (tool.ok ? " 成功" : " 失败") +
                " " + (tool.elapsedMs || 0) + "ms";
        }).join(" / ");
    }

    function historyEntry(message) {
        var isUser = message.role === "user";
        var ts = Number(message.ts);
        var time = ts ? new Date(ts).toLocaleTimeString("zh-CN", { hour12: false }) : "";
        return {
            kind: isUser ? "user" : "assistant",
            content: message.content || "",
            meta: (isUser ? "你" : "小明同学") + (time ? " · " + time : "") + " · 历史"
        };
    }

    async function loadHistory(roleId) {
        if (!invokeFn || historyLoaded[roleId]) {
            return;
        }
        historyLoaded[roleId] = true;

        try {
            var messages = await invokeFn("get_history", { role: roleId });
            if (!histories[roleId].length) {
                histories[roleId] = (messages || []).map(historyEntry);
            }
            if (roleId === currentRole) {
                renderHistory();
                updateStatus();
            }
        } catch (err) {
            historyLoaded[roleId] = false;
        }
    }

    function showPermission(request) {
        var requestId = request && (request.requestId || request.id);
        if (!requestId) {
            return;
        }
        if (activePermission) {
            permissionQueue.push(request);
            return;
        }

        activePermission = request;
        els.permLevel.textContent = request.level || "L2";
        els.permPath.textContent = request.target || request.path || "";
        els.permPreview.textContent = request.preview || "";
        els.permPanel.hidden = false;
        pendingConfirm += 1;
        updateStatus("等待写入确认");
    }

    function resolvePermission(action) {
        var request = activePermission;
        if (!request) {
            return;
        }

        var requestId = request.requestId || request.id;
        activePermission = null;
        els.permPanel.hidden = true;
        pendingConfirm = Math.max(0, pendingConfirm - 1);

        if (invokeFn) {
            invokeFn("confirm_permission", {
                requestId: requestId,
                action: action
            }).catch(function (err) {
                var message = err && err.message ? err.message : String(err);
                updateStatus("确认失败：" + message);
            });
        }

        if (action === "deny") {
            updateStatus("已拒绝写入");
        } else if (action === "allow_file") {
            updateStatus("本会话已允许此文件写入");
        } else {
            updateStatus("已允许写入");
        }
        if (permissionQueue.length) {
            showPermission(permissionQueue.shift());
        }
    }

    function listenPermissions() {
        if (!tauri) {
            return;
        }
        var eventApi = tauri.event || (tauri.core && tauri.core.event);
        if (!eventApi || typeof eventApi.listen !== "function") {
            return;
        }
        eventApi.listen("permission_request", function (event) {
            showPermission(event && event.payload);
        }).catch(function () {
            /* event bridge unavailable */
        });
    }

    function taskStageLabel(stage) {
        var labels = {
            planning: "拆解中",
            planned: "已拆解",
            subtask_start: "执行中",
            subtask_complete: "子任务完成",
            rollback: "回滚",
            summarizing: "汇总中",
            done: "完成",
            failed: "失败"
        };
        return labels[stage] || stage || "进度";
    }

    function ensureTaskCard(taskId) {
        if (activeTaskCard && activeTaskId === taskId) {
            return activeTaskCard;
        }
        if (!els.history || taskRole !== currentRole) {
            return null;
        }
        var card = document.createElement("div");
        card.className = "task-card";
        card.dataset.taskId = taskId || "";

        var head = document.createElement("div");
        head.className = "task-head";
        var title = document.createElement("span");
        title.className = "task-title";
        title.textContent = "任务进度";
        var status = document.createElement("span");
        status.className = "task-status";
        status.textContent = "进行中";
        head.appendChild(title);
        head.appendChild(status);

        var steps = document.createElement("div");
        steps.className = "task-steps";
        card.appendChild(head);
        card.appendChild(steps);

        var typing = els.history.querySelector(".message.typing");
        if (typing) {
            els.history.insertBefore(card, typing);
        } else {
            els.history.appendChild(card);
        }
        activeTaskCard = card;
        activeTaskId = taskId || null;
        return card;
    }

    function onTaskProgress(progress) {
        if (!progress || !progress.taskId) {
            return;
        }
        if (taskRole && taskRole !== currentRole) {
            return;
        }
        var card = ensureTaskCard(progress.taskId);
        if (!card) {
            return;
        }

        var steps = card.querySelector(".task-steps");
        var row = document.createElement("div");
        row.className = "task-step task-" + (progress.status || "running");
        var badge = document.createElement("span");
        badge.className = "task-badge";
        badge.textContent = taskStageLabel(progress.stage);
        var text = document.createElement("span");
        text.className = "task-text";
        text.textContent = (progress.agent ? progress.agent + " · " : "") +
            (progress.message || "") +
            (progress.elapsedMs ? " (" + progress.elapsedMs + "ms)" : "");
        row.appendChild(badge);
        row.appendChild(text);
        steps.appendChild(row);

        if (progress.stage === "done" || progress.stage === "failed") {
            var status = card.querySelector(".task-status");
            if (status) {
                status.textContent = progress.stage === "done" ? "完成" : "失败";
            }
            card.classList.add(progress.stage === "done" ? "is-done" : "is-failed");
        }
        scrollToBottom();
    }

    function listenTaskProgress() {
        if (!tauri) {
            return;
        }
        var eventApi = tauri.event || (tauri.core && tauri.core.event);
        if (!eventApi || typeof eventApi.listen !== "function") {
            return;
        }
        eventApi.listen("task_progress", function (event) {
            onTaskProgress(event && event.payload);
        }).catch(function () {
            /* event bridge unavailable */
        });
    }

    function updateStatus(extra) {
        var router = invokeFn
            ? (realReady ? "●3B 路由" : "●3B 未连接")
            : "●3B 浏览器预览";
        var routeText = "判定=- | 置信度=-";
        if (routing) {
            routeText = "路由中…";
        } else if (lastRoute) {
            routeText = "判定=" + lastRoute.role + " | 置信度=" + formatConfidence(lastRoute.confidence);
            if (lastRoute.fallback) {
                routeText += " | 兜底";
            }
        }

        var modeText = mode === "real" ? "真实" : "Mock";
        var parts = [
            router,
            routeText,
            modeText + " 模式",
            agentCount + " Agents",
            pendingConfirm + " 待确认",
            "角色 " + currentRole,
            histories[currentRole].length + " 条"
        ];
        if (extra) {
            parts.push(extra);
        }
        els.statusBar.textContent = parts.join(" | ");

        if (lastRoute) {
            var details = "路由模型：" + (lastRoute.model || "-") +
                "\n路由耗时：" + (lastRoute.elapsedMs || 0) + "ms";
            if (lastRoute.reason) {
                details += "\n兜底原因：" + lastRoute.reason;
            }
            els.statusBar.title = details;
        } else {
            els.statusBar.title = "";
        }
    }

    function applyTheme(theme) {
        var next = theme === "dark" ? "dark" : "light";
        document.documentElement.dataset.theme = next;
        try {
            localStorage.setItem("ai-assistant-theme", next);
        } catch (err) {
            /* localStorage may be unavailable under file:// restrictions */
        }
        els.themeBtn.textContent = next === "dark" ? "切换浅色" : "切换深色";
    }

    function buildMessage(entry) {
        var wrapper = document.createElement("div");
        wrapper.className = "message " + entry.kind;

        var bubble = document.createElement("div");
        bubble.className = "bubble";
        bubble.textContent = entry.content;
        wrapper.appendChild(bubble);

        if (entry.meta) {
            var meta = document.createElement("div");
            meta.className = "meta";
            meta.textContent = entry.meta;
            wrapper.appendChild(meta);
        }

        return wrapper;
    }

    function renderHistory() {
        els.history.innerHTML = "";
        var list = histories[currentRole];

        if (!list.length) {
            var empty = document.createElement("div");
            empty.className = "empty-state";
            var title = document.createElement("strong");
            title.textContent = roleName(currentRole);
            var hint = document.createElement("div");
            var role = roleById(currentRole);
            hint.textContent = (role && role.emptyHint) || "发一条消息试试。";
            empty.appendChild(title);
            empty.appendChild(hint);
            els.history.appendChild(empty);
            return;
        }

        for (var i = 0; i < list.length; i += 1) {
            els.history.appendChild(buildMessage(list[i]));
        }
        scrollToBottom();
    }

    function addMessage(roleId, kind, content, meta) {
        var entry = { kind: kind, content: content, meta: meta || "" };
        histories[roleId].push(entry);
        if (roleId !== currentRole) {
            return;
        }
        var empty = els.history.querySelector(".empty-state");
        if (empty) {
            empty.remove();
        }
        els.history.appendChild(buildMessage(entry));
        scrollToBottom();
    }

    function showTyping() {
        var wrapper = document.createElement("div");
        wrapper.className = "message assistant typing";
        var bubble = document.createElement("div");
        bubble.className = "bubble";
        for (var i = 0; i < 3; i += 1) {
            var dot = document.createElement("span");
            dot.className = "dot";
            bubble.appendChild(dot);
        }
        wrapper.appendChild(bubble);
        els.history.appendChild(wrapper);
        scrollToBottom();
        return wrapper;
    }

    function renderRoles() {
        els.roleList.innerHTML = "";
        roles.forEach(function (role) {
            var item = document.createElement("li");
            item.className = "role-item" + (role.id === currentRole ? " active" : "");
            item.dataset.role = role.id;

            var name = document.createElement("span");
            name.className = "role-name";
            name.textContent = role.name;

            var desc = document.createElement("span");
            desc.className = "role-desc";
            desc.textContent = role.desc;

            item.appendChild(name);
            item.appendChild(desc);
            item.addEventListener("click", function () {
                switchRole(role.id);
            });
            els.roleList.appendChild(item);
        });
    }

    function renderRole() {
        var role = roleById(currentRole);
        els.roleTitle.textContent = role ? role.name : currentRole;
        els.roleDesc.textContent = role ? role.desc : "";
        var items = els.roleList.querySelectorAll(".role-item");
        for (var i = 0; i < items.length; i += 1) {
            items[i].classList.toggle("active", items[i].dataset.role === currentRole);
        }
        document.title = "小明同学 · " + roleName(currentRole);
    }

    function switchRole(roleId) {
        if (!histories[roleId] || roleId === currentRole) {
            return;
        }
        currentRole = roleId;
        renderRole();
        renderHistory();
        updateStatus();
        loadHistory(roleId);
    }

    function mockReply(roleId, text) {
        return "[Mock] " + roleName(roleId) + " 收到：" + text +
            "\n\n这是模拟回复。切换到「真实」模式可调用本地 qwen25-3b。";
    }

    function requestRoute(text) {
        if (!invokeFn) {
            return;
        }

        routeSeq += 1;
        var seq = routeSeq;
        routing = true;
        updateStatus();

        invokeFn("route_message", { content: text }).then(function (decision) {
            if (seq !== routeSeq) {
                return;
            }
            routing = false;
            lastRoute = decision || {
                role: "general",
                intent: "unknown",
                confidence: 0,
                fallback: true,
                reason: "empty_result"
            };
            updateStatus();
        }).catch(function () {
            if (seq !== routeSeq) {
                return;
            }
            routing = false;
            lastRoute = {
                role: "general",
                intent: "unknown",
                confidence: 0,
                fallback: true,
                reason: "ipc_error"
            };
            updateStatus();
        });
    }

    async function sendMessage() {
        var text = els.input.value.trim();
        if (!text || sending) {
            return;
        }

        var roleAtSend = currentRole;
        taskRole = roleAtSend;
        activeTaskCard = null;
        activeTaskId = null;
        els.input.value = "";
        addMessage(roleAtSend, "user", text, "你 · " + timeText());
        sending = true;
        els.sendBtn.disabled = true;
        els.sendBtn.textContent = "发送中…";
        updateStatus();
        requestRoute(text);

        var typing = showTyping();
        var started = window.performance.now();

        try {
            var content;
            var meta;

            if (mode === "real" && realReady && invokeFn) {
                var reply = await invokeFn("send_message", {
                    role: roleAtSend,
                    content: text
                });
                content = reply.content;
                meta = "小明同学 · " + reply.model + " · " + reply.elapsedMs + "ms" +
                    toolSummary(reply.toolCalls);
            } else {
                await sleep(350);
                content = mockReply(roleAtSend, text);
                meta = "小明同学 · mock · " + Math.round(window.performance.now() - started) + "ms";
            }

            typing.remove();
            addMessage(roleAtSend, "assistant", content, meta);
            updateStatus();
        } catch (err) {
            typing.remove();
            var message = err && err.message ? err.message : String(err);
            addMessage(roleAtSend, "error", "调用失败：" + message, "小明同学 · " + timeText());
            updateStatus("调用失败");
        } finally {
            sending = false;
            els.sendBtn.disabled = false;
            els.sendBtn.textContent = "发送";
            els.input.focus();
        }
    }

    function clearCurrent() {
        histories[currentRole] = [];
        historyLoaded[currentRole] = true;
        routeSeq += 1;
        routing = false;
        lastRoute = null;

        if (invokeFn) {
            invokeFn("clear_history", { role: currentRole }).catch(function () {
                /* keep the UI clear even if persistence cleanup fails */
            });
        }

        renderHistory();
        updateStatus();
    }

    function bindEvents() {
        els.sendBtn.addEventListener("click", sendMessage);
        els.clearBtn.addEventListener("click", clearCurrent);
        els.permApprove.addEventListener("click", function () {
            resolvePermission("allow");
        });
        els.permAllowFile.addEventListener("click", function () {
            resolvePermission("allow_file");
        });
        els.permDeny.addEventListener("click", function () {
            resolvePermission("deny");
        });
        els.envRetry.addEventListener("click", function () {
            envDismissed = false;
            checkEnvironment();
        });
        els.envDismiss.addEventListener("click", function () {
            envDismissed = true;
            if (els.envBanner) {
                els.envBanner.hidden = true;
            }
        });

        els.input.addEventListener("keydown", function (event) {
            if (event.key === "Enter" && !event.shiftKey) {
                event.preventDefault();
                sendMessage();
            }
        });

        els.themeBtn.addEventListener("click", function () {
            var next = document.documentElement.dataset.theme === "dark" ? "light" : "dark";
            applyTheme(next);
        });

        els.modeSelect.addEventListener("change", function () {
            var next = els.modeSelect.value;
            if (next === "real" && !realReady) {
                els.modeSelect.value = "mock";
                mode = "mock";
                updateStatus("真实模式不可用");
                return;
            }
            mode = next;
            updateStatus();
        });
    }

    function renderEnvironment(report) {
        if (!els.envBanner) {
            return;
        }
        if (!report || report.ok) {
            els.envBanner.hidden = true;
            return;
        }
        if (envDismissed) {
            els.envBanner.hidden = true;
            return;
        }

        els.envTitle.textContent = report.ollamaReachable ? "缺少本地模型" : "未连接到 Ollama";
        var guidance = Array.isArray(report.guidance) ? report.guidance : [];
        if (!guidance.length && report.error) {
            guidance = [report.error];
        }
        els.envDetail.textContent = guidance.join("\n");
        els.envBanner.hidden = false;
    }

    async function checkEnvironment() {
        if (!invokeFn) {
            return;
        }
        try {
            var report = await invokeFn("check_environment");
            realReady = Boolean(report && report.ok);
            var realOption = els.modeSelect.querySelector('option[value="real"]');
            if (realOption) {
                realOption.disabled = !realReady;
            }
            if (!realReady && mode === "real") {
                mode = "mock";
                els.modeSelect.value = "mock";
            }
            renderEnvironment(report);
            if (realReady) {
                updateStatus("Ollama 就绪");
            }
        } catch (err) {
            realReady = false;
            var realOption = els.modeSelect.querySelector('option[value="real"]');
            if (realOption) {
                realOption.disabled = true;
            }
            if (mode === "real") {
                mode = "mock";
                els.modeSelect.value = "mock";
            }
            renderEnvironment({
                ok: false,
                ollamaReachable: false,
                guidance: ["环境检测失败：" + (err && err.message ? err.message : String(err))]
            });
        }
    }

    async function probeBackend() {
        if (!invokeFn) {
            els.backendInfo.textContent = "浏览器预览";
            updateStatus("Mock 可用");
            return;
        }

        try {
            var status = await invokeFn("get_status");
            realReady = Boolean(status && status.ollamaReady);
            agentCount = Number(status && status.agents) || 0;
            pendingConfirm = Number(status && status.pendingConfirm) || 0;
            var realOption = els.modeSelect.querySelector('option[value="real"]');
            if (realOption) {
                realOption.disabled = !realReady;
            }

            if (realReady) {
                els.backendInfo.textContent = status.routerModel + " / " + status.coordinatorModel;
                els.backendInfo.title = "Ollama: " + status.ollamaBaseUrl + "\n数据目录: " + status.dataDir;
                updateStatus("Ollama " + status.ollamaBaseUrl);
            } else {
                var missing = Array.isArray(status.missingModels) ? status.missingModels : [];
                els.backendInfo.textContent = missing.length ? "缺少模型：" + missing.join("、") : "Ollama 未连接";
                els.backendInfo.title = "Ollama: " + status.ollamaBaseUrl;
                updateStatus(missing.length ? "请拉取缺失模型" : "请先启动 Ollama");
            }
        } catch (err) {
            realReady = false;
            els.backendInfo.textContent = "后端不可用";
            updateStatus("get_status 调用失败");
        }
    }

    function init() {
        els.roleList = $("role-list");
        els.roleTitle = $("role-title");
        els.roleDesc = $("role-desc");
        els.history = $("chat-history");
        els.input = $("input-text");
        els.sendBtn = $("send-btn");
        els.clearBtn = $("clear-btn");
        els.statusBar = $("status-bar");
        els.themeBtn = $("theme-btn");
        els.modeSelect = $("mode-select");
        els.backendInfo = $("backend-info");
        els.permPanel = $("permission-panel");
        els.permLevel = $("perm-level");
        els.permPath = $("perm-path");
        els.permPreview = $("perm-preview");
        els.permApprove = $("perm-approve");
        els.permAllowFile = $("perm-allow-file");
        els.permDeny = $("perm-deny");
        els.envBanner = $("env-banner");
        els.envTitle = $("env-title");
        els.envDetail = $("env-detail");
        els.envRetry = $("env-retry");
        els.envDismiss = $("env-dismiss");

        roles.forEach(function (role) {
            histories[role.id] = [];
        });

        var savedTheme = "light";
        try {
            savedTheme = localStorage.getItem("ai-assistant-theme") || "light";
        } catch (err) {
            savedTheme = "light";
        }

        applyTheme(savedTheme);
        renderRoles();
        renderRole();
        renderHistory();
        bindEvents();
        listenPermissions();
        listenTaskProgress();
        updateStatus();
        loadHistory(currentRole);
        probeBackend().then(function () {
            checkEnvironment();
        });
    }

    if (document.readyState === "loading") {
        document.addEventListener("DOMContentLoaded", init);
    } else {
        init();
    }
})();
