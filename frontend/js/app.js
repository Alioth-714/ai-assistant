(function () {
    "use strict";

    var roles = Array.isArray(window.ROLES) ? window.ROLES.slice() : [];
    var tauri = window.__TAURI__ || null;
    var invokeFn = null;
    if (tauri) {
        if (typeof tauri.invoke === "function") {
            invokeFn = tauri.invoke.bind(tauri);
        } else if (tauri.core && typeof tauri.core.invoke === "function") {
            invokeFn = tauri.core.invoke.bind(tauri.core);
        }
    }

    var sessionsByRole = {};
    var currentSessionByRole = {};
    var messagesBySession = {};
    var loadedSessions = {};
    var currentRole = roles.length ? roles[0].id : "dev";
    var mode = "mock";
    var realReady = false;
    var sending = false;
    var agentCount = 0;
    var pendingConfirm = 0;
    var routing = false;
    var lastRoute = null;
    var routeSeq = 0;
    var permissionQueue = [];
    var activePermission = null;
    var activeTaskCard = null;
    var activeTaskId = null;
    var taskRole = null;
    var envDismissed = false;
    var roleWheelOpen = false;
    var roleWheelTimer = null;
    var roleWheelSuppress = false;
    var mockSessionSeq = 0;
    var settings = null;
    var settingsView = null;
    // 决策 33：「仅本地」开关状态与本次会话云端次数
    var cloudEscalations = 0;
    var onboardingStep = 0;
    var onboardingShown = false;
    var pendingImport = null;
    var providerPresets = [];
    var lastGenerateError = "";
    var pendingDataRoot = "";
    var dataSwitchBusy = false;
    var restartNoticeTimer = null;
    var restartNoticeRemaining = 0;
    var restartFallbackActive = false;
    var switchedTarget = "";
    var switchedFiles = 0;
    var localModels = [];
    var modelOptions = [];
    var editingRoleId = null;
    var providers = [];
    var providerPickerContext = null;
    var exportContext = null;
    var lastUsage = {};
    var toastTimer = null;
    var hintCountBySession = {};
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

    function normalizeRole(item) {
        return {
            id: item.id,
            name: item.name || item.id,
            desc: item.desc || "",
            emptyHint: item.desc || "发一条消息试试。",
            systemPrompt: item.systemPrompt || "",
            modelPreference: item.modelPreference || "heavy",
            builtin: Boolean(item.builtin),
            memoryScope: item.memoryScope || "full",
            cloudPolicy: item.cloudPolicy,
            cloudPolicyResolved: item.cloudPolicyResolved || "",
            disclaimer: item.disclaimer || ""
        };
    }

    async function loadRoles() {
        if (!invokeFn) {
            return;
        }
        try {
            var list = await invokeFn("list_roles");
            if (!Array.isArray(list) || !list.length) {
                return;
            }
            roles = list.map(normalizeRole);
            roles.forEach(function (role) {
                if (!sessionsByRole[role.id]) {
                    sessionsByRole[role.id] = [];
                }
            });
            var changed = !roleById(currentRole);
            if (changed) {
                currentRole = roles[0].id;
            }
            renderRole();
            renderSessions();
            renderHistory();
            updateStatus();
            if (changed) {
                loadSessions(currentRole);
            }
        } catch (err) {
            /* keep the bundled roles when the backend is unavailable */
        }
    }

    /// 取路径最后一段（用于只显示目录名，不铺长路径）
    function baseName(path) {
        if (!path) {
            return "";
        }
        var parts = String(path).split(/[\\/]+/).filter(function (item) {
            return item;
        });
        return parts.length ? parts[parts.length - 1] : String(path);
    }

    function roleName(id) {        var role = roleById(id);
        return role ? role.name : id;
    }

    function roleIndex() {
        for (var i = 0; i < roles.length; i += 1) {
            if (roles[i].id === currentRole) {
                return i;
            }
        }
        return 0;
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

    function relativeTime(ts) {
        var value = Number(ts);
        if (!value) {
            return "刚刚";
        }
        var diff = Date.now() - value;
        if (diff < 60 * 1000) {
            return "刚刚";
        }
        if (diff < 60 * 60 * 1000) {
            return Math.floor(diff / 60000) + " 分钟前";
        }
        if (diff < 24 * 60 * 60 * 1000) {
            return Math.floor(diff / 3600000) + " 小时前";
        }
        var date = new Date(value);
        return (date.getMonth() + 1) + "-" + date.getDate() + " " +
            date.toLocaleTimeString("zh-CN", { hour12: false, hour: "2-digit", minute: "2-digit" });
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

    function currentSessionId() {
        return currentSessionByRole[currentRole] || null;
    }

    function currentMessages() {
        var sessionId = currentSessionId();
        return sessionId ? (messagesBySession[sessionId] || []) : [];
    }

    function sessionById(roleId, sessionId) {
        var list = sessionsByRole[roleId] || [];
        for (var i = 0; i < list.length; i += 1) {
            if (list[i].sessionId === sessionId) {
                return list[i];
            }
        }
        return null;
    }

    function historyEntry(message) {
        var isUser = message.role === "user";
        var ts = Number(message.ts);
        var time = ts ? new Date(ts).toLocaleTimeString("zh-CN", { hour12: false }) : "";
        var meta = (isUser ? "你" : "小明同学") + (time ? " · " + time : "");
        if (!isUser && message.model) {
            meta += " · " + message.model;
        }
        if (!isUser && message.sensitive) {
            meta += " · 含文件内容";
        }
        return {
            kind: isUser ? "user" : "assistant",
            content: message.content || "",
            meta: meta
        };
    }

    function ensureMockSession(roleId) {
        if (!sessionsByRole[roleId]) {
            sessionsByRole[roleId] = [];
        }
        if (sessionsByRole[roleId].length) {
            return;
        }
        mockSessionSeq += 1;
        var now = Date.now();
        var sessionId = "mock-" + roleId + "-" + mockSessionSeq;
        sessionsByRole[roleId].push({
            sessionId: sessionId,
            role: roleId,
            title: "示例对话",
            createdAt: now,
            updatedAt: now,
            messageCount: 0,
            hasSensitive: false
        });
        messagesBySession[sessionId] = [];
        loadedSessions[sessionId] = true;
        if (!currentSessionByRole[roleId]) {
            currentSessionByRole[roleId] = sessionId;
        }
    }

    async function loadSessions(roleId) {
        if (!invokeFn) {
            ensureMockSession(roleId);
            if (roleId === currentRole) {
                renderSessions();
                renderHistory();
                updateStatus();
            }
            return;
        }

        try {
            var list = await invokeFn("list_sessions", { role: roleId });
            sessionsByRole[roleId] = Array.isArray(list) ? list : [];
            var current = currentSessionByRole[roleId];
            var exists = current && sessionsByRole[roleId].some(function (session) {
                return session.sessionId === current;
            });
            if (!exists) {
                currentSessionByRole[roleId] = sessionsByRole[roleId].length
                    ? sessionsByRole[roleId][0].sessionId
                    : null;
            }
            if (roleId === currentRole) {
                renderSessions();
                var active = currentSessionByRole[roleId];
                if (active) {
                    await loadSessionMessages(roleId, active);
                } else {
                    renderHistory();
                }
                updateStatus();
            }
        } catch (err) {
            updateStatus("加载会话失败");
        }
    }

    async function loadSessionMessages(roleId, sessionId) {
        if (!sessionId) {
            renderHistory();
            return;
        }
        if (!invokeFn) {
            if (!messagesBySession[sessionId]) {
                messagesBySession[sessionId] = [];
            }
            if (roleId === currentRole) {
                renderHistory();
                updateStatus();
            }
            return;
        }
        if (loadedSessions[sessionId]) {
            if (roleId === currentRole) {
                renderHistory();
                updateStatus();
            }
            return;
        }

        try {
            var detail = await invokeFn("get_session", { role: roleId, sessionId: sessionId });
            messagesBySession[sessionId] = ((detail && detail.messages) || []).map(historyEntry);
            loadedSessions[sessionId] = true;
            if (roleId === currentRole) {
                renderHistory();
                updateStatus();
            }
        } catch (err) {
            updateStatus("加载会话失败");
        }
    }

    async function createSession(roleId) {
        cloudEscalations = 0;
        if (!invokeFn) {
            ensureMockSession(roleId);
            mockSessionSeq += 1;
            var now = Date.now();
            var sessionId = "mock-" + roleId + "-" + mockSessionSeq;
            sessionsByRole[roleId].unshift({
                sessionId: sessionId,
                role: roleId,
                title: "新对话",
                createdAt: now,
                updatedAt: now,
                messageCount: 0,
                hasSensitive: false
            });
            currentSessionByRole[roleId] = sessionId;
            messagesBySession[sessionId] = [];
            loadedSessions[sessionId] = true;
            renderSessions();
            renderHistory();
            updateStatus();
            return;
        }

        try {
            var created = await invokeFn("create_session", { role: roleId });
            sessionsByRole[roleId] = [created].concat(sessionsByRole[roleId] || []);
            currentSessionByRole[roleId] = created.sessionId;
            messagesBySession[created.sessionId] = [];
            loadedSessions[created.sessionId] = true;
            renderSessions();
            renderHistory();
            updateStatus();
        } catch (err) {
            updateStatus("新建会话失败");
        }
    }

    async function selectSession(roleId, sessionId) {
        currentSessionByRole[roleId] = sessionId;
        renderSessions();
        await loadSessionMessages(roleId, sessionId);
        renderHistory();
        updateStatus();
    }

    async function renameSession(roleId, sessionId) {
        var session = sessionById(roleId, sessionId);
        var next = window.prompt("重命名对话", session ? session.title : "");
        if (next === null) {
            return;
        }
        next = next.trim();
        if (!next) {
            return;
        }
        if (!invokeFn) {
            if (session) {
                session.title = next;
            }
            renderSessions();
            return;
        }

        try {
            await invokeFn("rename_session", {
                role: roleId,
                sessionId: sessionId,
                title: next
            });
            if (session) {
                session.title = next;
            }
            renderSessions();
        } catch (err) {
            updateStatus("重命名失败");
        }
    }

    async function deleteSession(roleId, sessionId) {
        if (!window.confirm("删除这条对话？")) {
            return;
        }
        if (!invokeFn) {
            sessionsByRole[roleId] = (sessionsByRole[roleId] || []).filter(function (session) {
                return session.sessionId !== sessionId;
            });
            delete messagesBySession[sessionId];
            delete loadedSessions[sessionId];
            if (currentSessionByRole[roleId] === sessionId) {
                currentSessionByRole[roleId] = sessionsByRole[roleId].length
                    ? sessionsByRole[roleId][0].sessionId
                    : null;
            }
            renderSessions();
            renderHistory();
            updateStatus();
            return;
        }

        try {
            await invokeFn("delete_session", { role: roleId, sessionId: sessionId });
            sessionsByRole[roleId] = (sessionsByRole[roleId] || []).filter(function (session) {
                return session.sessionId !== sessionId;
            });
            delete messagesBySession[sessionId];
            delete loadedSessions[sessionId];
            if (currentSessionByRole[roleId] === sessionId) {
                currentSessionByRole[roleId] = sessionsByRole[roleId].length
                    ? sessionsByRole[roleId][0].sessionId
                    : null;
            }
            renderSessions();
            if (currentSessionByRole[roleId]) {
                await loadSessionMessages(roleId, currentSessionByRole[roleId]);
            }
            renderHistory();
            updateStatus();
        } catch (err) {
            updateStatus("删除失败");
        }
    }

    function renderSessions() {
        if (!els.sessionList) {
            return;
        }
        els.sessionList.innerHTML = "";
        var role = roleById(currentRole);
        els.sessionRole.textContent = role ? role.name : currentRole;
        var list = sessionsByRole[currentRole] || [];
        if (!list.length) {
            var empty = document.createElement("div");
            empty.className = "session-empty";
            empty.textContent = "还没有对话，点 + 新对话开始。";
            els.sessionList.appendChild(empty);
            return;
        }

        var activeId = currentSessionByRole[currentRole];
        list.forEach(function (session) {
            var item = document.createElement("div");
            item.className = "session-item" + (session.sessionId === activeId ? " active" : "");
            item.dataset.session = session.sessionId;

            var title = document.createElement("span");
            title.className = "session-title";
            title.textContent = session.title || "新对话";

            var meta = document.createElement("span");
            meta.className = "session-meta";
            meta.textContent = (session.messageCount || 0) + " 条 · " + relativeTime(session.updatedAt);

            var actions = document.createElement("div");
            actions.className = "session-actions";

            var rename = document.createElement("button");
            rename.type = "button";
            rename.className = "session-action";
            rename.textContent = "改名";
            rename.addEventListener("click", function (event) {
                event.stopPropagation();
                renameSession(currentRole, session.sessionId);
            });

            var remove = document.createElement("button");
            remove.type = "button";
            remove.className = "session-action";
            remove.textContent = "删除";
            remove.addEventListener("click", function (event) {
                event.stopPropagation();
                deleteSession(currentRole, session.sessionId);
            });

            actions.appendChild(rename);
            actions.appendChild(remove);
            item.appendChild(title);
            item.appendChild(meta);
            item.appendChild(actions);
            item.addEventListener("click", function () {
                selectSession(currentRole, session.sessionId);
            });
            els.sessionList.appendChild(item);
        });
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
            ? (realReady ? ("●" + localModelLabel()) : "●本地模型未连接")
            : "●本地模型（浏览器预览）";
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
        var sessionId = currentSessionId();
        var session = sessionId ? sessionById(currentRole, sessionId) : null;
        var parts = [
            router,
            routeText,
            autoEscalateOn() ? "自动·云端" : "仅本地",
            modeText + " 模式",
            agentCount + " Agents",
            pendingConfirm + " 待确认",
            "角色 " + currentRole,
            session ? (session.title || "新对话") : "无会话",
            currentMessages().length + " 条",
            "云端 " + cloudEscalations + " 次"
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

    // ---- 决策 33：「仅本地」开关 ----
    function autoEscalateOn() {
        return !!(settings && settings.cloud && settings.cloud.autoEscalate);
    }

    /// 主界面「云端供应商」卡片：显示当前供应商余额/状态，60 秒刷新一次
    var localMetaText = "";
    var usageTimer = null;
    var usageCache = { value: "", note: "", at: 0 };

    /// 主界面「今日用量」卡片：云端/本地汇总 + 勾选的供应商额度
    async function refreshUsageCard() {
        var cloudEl = document.getElementById("usage-cloud-line");
        var localEl = document.getElementById("usage-local-line");
        var blocksEl = document.getElementById("usage-blocks");
        var updatedEl = document.getElementById("usage-updated");
        if (!cloudEl || !blocksEl) {
            return;
        }
        function money(value) {
            return "¥" + Number(value || 0).toFixed(2);
        }
        try {
            var total = await invokeFn("usage_summary", { period: "today" });
            var local = await invokeFn("usage_summary", { period: "today", providerId: "local" });
            var cloud = {
                cost: Math.max(0, (total.cost || 0) - (local.cost || 0)),
                requests: Math.max(0, (total.requests || 0) - (local.requests || 0)),
                tokensIn: Math.max(0, (total.tokensIn || 0) - (local.tokensIn || 0)),
                tokensOut: Math.max(0, (total.tokensOut || 0) - (local.tokensOut || 0))
            };
            cloudEl.textContent = cloud.requests === 0 ? "云端  今日暂无调用" : "云端  " + money(cloud.cost) + " · " + cloud.requests + " 次 · 输入 " +
                Math.round(cloud.tokensIn / 1000) + "k / 输出 " + Math.round(cloud.tokensOut / 1000) + "k";
            localMetaText = (local.requests || 0) + " 次 · 约 " +
                Math.round(((local.tokensIn || 0) + (local.tokensOut || 0)) / 1000) + "k tokens";



            localEl.textContent = (local.requests || 0) === 0 ? "本地  今日暂无调用" : "本地  约 " +
                Math.round(((local.tokensIn || 0) + (local.tokensOut || 0)) / 1000) + "k tokens · " +
                local.requests + " 次";
        } catch (err) {
            cloudEl.textContent = "云端数据不可用";
            localEl.textContent = "本地数据不可用";
        }
        if (updatedEl) {
            var now = new Date();
            updatedEl.textContent = now.getHours().toString().padStart(2, "0") + ":" +
                now.getMinutes().toString().padStart(2, "0");
        }
        blocksEl.textContent = "";
        if (!localMetaText) { localMetaText = "—"; }
        var localBlock = document.createElement("div");
        localBlock.className = "usage-block";
        var localHead = document.createElement("div");
        localHead.className = "usage-block-head";
        localHead.textContent = "本地（Ollama）";
        var localMeta = document.createElement("div");
        localMeta.className = "usage-provider-note";
        localMeta.id = "usage-local-meta";
        localMeta.textContent = localMetaText;
        localBlock.appendChild(localHead);
        localBlock.appendChild(localMeta);
        blocksEl.appendChild(localBlock);

        var ids = (settings && settings.usageProviders) || [];
        if (!ids.length) {
            var hint = document.createElement("div");
            hint.className = "usage-provider-note";
            hint.textContent = "未选择供应商，可在「设置-费用」勾选。";
            blocksEl.appendChild(hint);
            return;
        }
        for (var i = 0; i < ids.length; i += 1) {
            var provider = providerById(ids[i]);
            if (!provider) {
                continue;
            }
            var block = document.createElement("div");
            block.className = "usage-block";
            var head = document.createElement("div");
            head.className = "usage-block-head";
            head.textContent = provider.name;
            block.appendChild(head);
            var body = document.createElement("div");
            body.className = "usage-provider-note";
            body.textContent = "查询中…";
            block.appendChild(body);
            blocksEl.appendChild(block);
            if (provider.hasApiKey) {
                fetchProviderQuota(provider, body);
            } else {
                body.textContent = "未配置 API Key";
            }
        }
    }
    function startUsageTimer() {
        refreshUsageCard();
        if (usageTimer) {
            window.clearInterval(usageTimer);
        }
        var minutes = (settings && settings.usageRefreshMinutes) || 5;
        minutes = Math.min(60, Math.max(1, minutes));
        usageTimer = window.setInterval(refreshUsageCard, minutes * 60000);
    }
    function renderLocalOnlyToggle() {
        if (!els.localOnlyBtn) {
            return;
        }
        var auto = autoEscalateOn();
        els.localOnlyBtn.classList.toggle("on", !auto);
        els.localOnlyBtn.classList.toggle("off", auto);
        els.localOnlyBtn.setAttribute("aria-pressed", auto ? "false" : "true");
        els.localOnlyBtn.innerHTML = '<span class="dot"></span>' + (auto ? "自动·云端" : "仅本地");
        els.localOnlyBtn.title = auto
            ? "自动模式：复杂任务直接使用云端，不再逐次询问（敏感内容仍受红线保护）。点击切回仅本地"
            : "仅本地：只跑本地模型；需要云端协助时会弹窗确认。点击切到自动模式";
    }

    /// 状态栏显示的本地模型名（取当前协调模型，未配置时回退通用文案）
    function localModelLabel() {
        var name = settings && settings.models && settings.models.coordinator;
        return name ? "本地 · " + name : "本地模型";
    }
    /// 当前使用的模型引用（全局默认，聊天框与设置页共用）
    /// 是否处于「自动·云端」模式（与底部开关同一来源）
    function isAutoCloud() {
        return autoEscalateOn();
    }
    function roleModelRef() {
        var s = settings || {};
        return s.defaultChat || "local";
    }
    /// 从 modelRef 取供应商 id（"local" 或 "provider:model"）
    function providerOfRef(ref) {
        var text = String(ref || "");
        var idx = text.indexOf(":");
        return idx > 0 ? text.slice(0, idx) : text;
    }

    /// 模型名（不带供应商前缀）
    function modelNameOnly(ref) {
        if (!ref || ref === "local") {
            var coordinator = settings && settings.models ? settings.models.coordinator : "";
            return coordinator || "本地模型";
        }
        var item = ((settings && settings.externalModels) || []).filter(function (entry) {
            return entry.id === ref;
        })[0];
        return item ? (item.displayName || item.modelId) : ref;
    }
    function composerModelLabel(ref) {
        if (!ref || ref === "local") {
            var coordinator = settings && settings.models ? settings.models.coordinator : "";
            return coordinator ? "本地 · " + coordinator : "本地模型";
        }
        var item = ((settings && settings.externalModels) || []).filter(function (entry) {
            return entry.id === ref;
        })[0];
        if (!item) {
            return ref;
        }
        var provider = providerById(item.providerId);
        return (provider ? provider.name + " · " : "") + (item.displayName || item.modelId);
    }

    /// 弹窗顶部提示：当前模式与已选模型冲突时给出说明与跳转
    function renderComposerModeBanner(cloudOnly, autoMode, currentRef) {
        var banner = document.getElementById("composer-mode-banner");
        if (!banner) {
            return;
        }
        banner.textContent = "";
        var text = "";
        if (cloudOnly && currentRef === "local") {
            text = "当前为「仅云端」模式，本地模型不可用。";
        } else if (!autoMode && currentRef !== "local") {
            text = "当前为「仅本地」模式，云端模型不可选。";
        }
        if (!text) {
            banner.hidden = true;
            return;
        }
        banner.hidden = false;
        var span = document.createElement("span");
        span.textContent = text;
        banner.appendChild(span);
        var jump = document.createElement("button");
        jump.type = "button";
        jump.className = "ghost";
        jump.textContent = "去切换模式";
        jump.addEventListener("click", function () {
            closeComposerModelPicker();
            openSettings("external");
        });
        banner.appendChild(jump);
    }
    var pendingModelRef = "";
    function openComposerModelPicker() {
        var modal = document.getElementById("composer-model-modal");
        var list = document.getElementById("composer-model-list");
        if (!modal || !list) {
            return;
        }
        list.textContent = "";
        var currentRef = roleModelRef();
        var autoMode = isAutoCloud();
        uiLog("[mode] cloud.mode=" + ((settings && settings.cloud && settings.cloud.mode) || "") +
            " autoEscalate=" + ((settings && settings.cloud && settings.cloud.autoEscalate) ? "true" : "false") +
            " autoMode=" + (autoMode ? "true" : "false"));
        var cloudOnly = Boolean(settings && settings.cloud && settings.cloud.mode === "cloudOnly");

        function addHeader(text) {
            var header = document.createElement("div");
            header.className = "provider-group-head";
            header.textContent = text;
            list.appendChild(header);
        }

        function addRow(ref, label, unavailable, reason) {
            var row = document.createElement("div");
            row.className = "composer-model-item" + (currentRef === ref ? " active" : "");
            if (unavailable) {
                row.classList.add("disabled");
            }
            var name = document.createElement("span");
            name.textContent = label;
            row.appendChild(name);
            if (unavailable && reason) {
                var why = document.createElement("span");
                why.className = "disabled-reason";
                why.textContent = reason;
                row.appendChild(why);
            }
            if (currentRef === ref) {
                var check = document.createElement("span");
                check.className = "composer-check";
                check.textContent = "✓";
                row.appendChild(check);
            }
            row.addEventListener("click", function () {
                if (unavailable) {
                    return;
                }
                chooseComposerModel(ref);
            });
            list.appendChild(row);
        }

        // 本地组固定最后；先渲染供应商分组
        var models = ((settings && settings.externalModels) || []).filter(function (item) {
            return item.enabled !== false;
        });
        var providerOrder = (settings && settings.providerOrder) || [];
        var providerIds = [];
        models.forEach(function (item) {
            if (providerIds.indexOf(item.providerId) === -1) {
                providerIds.push(item.providerId);
            }
        });
        providerIds.sort(function (a, b) {
            var ia = providerOrder.indexOf(a);
            var ib = providerOrder.indexOf(b);
            return (ia < 0 ? 999 : ia) - (ib < 0 ? 999 : ib);
        });
        providerIds.forEach(function (providerId) {
            var provider = providerById(providerId);
            addHeader(provider ? provider.name : providerId);
            var order = (((settings || {}).modelOrder || {})[providerId]) || [];
            var items = models.filter(function (item) {
                return item.providerId === providerId;
            }).sort(function (a, b) {
                var ia = order.indexOf(a.modelId);
                var ib = order.indexOf(b.modelId);
                if (ia < 0 && ib < 0) {
                    return 0;
                }
                return (ia < 0 ? 9999 : ia) - (ib < 0 ? 9999 : ib);
            });
            items.forEach(function (item) {
                addRow(item.id, item.displayName || item.modelId, !autoMode, "仅本地模式");
            });
        });

        addHeader("本地 ollama");
        var coordinator = settings && settings.models ? settings.models.coordinator : "";
        addRow("local", coordinator || "本地模型", cloudOnly, "仅云端模式");

        renderComposerModeBanner(cloudOnly, autoMode, currentRef);
        renderComposerLevelRow(currentRef);
        modal.hidden = false;
    }

    /// 弹窗底部的推理等级行（仅当模型声明了等级时出现）
    function renderComposerLevelRow(ref) {
        var host = document.getElementById("composer-level-row");
        if (!host) {
            return;
        }
        host.textContent = "";
        var levels = modelLevelsOf(ref);
        if (!levels.length) {
            host.hidden = true;
            return;
        }
        host.hidden = false;
        var label = document.createElement("span");
        label.className = "composer-level-label";
        label.textContent = "等级：";
        host.appendChild(label);
        var current = currentModelLevel(ref);
        ["默认"].concat(levels).forEach(function (level) {
            var value = level === "默认" ? "" : level;
            var btn = document.createElement("button");
            btn.type = "button";
            btn.className = "ghost composer-level" + (current === value ? " active" : "");
            btn.textContent = level;
            btn.addEventListener("click", function () {
                chooseComposerLevel(ref, value);
            });
            host.appendChild(btn);
        });
    }

    /// 模型支持的等级列表（来自预设/配置，暂为空表示不支持）
    function modelLevelsOf(ref) {
        var item = ((settings && settings.externalModels) || []).filter(function (entry) {
            return entry.id === ref;
        })[0];
        if (!item) {
            return [];
        }
        return Array.isArray(item.reasoningLevels) ? item.reasoningLevels : [];
    }

    function currentModelLevel(ref) {
        var map = (settings && settings.modelLevels) || {};
        return map[ref] || "";
    }

    async function chooseComposerLevel(ref, level) {
        if (!settings) {
            return;
        }
        settings.modelLevels = settings.modelLevels || {};
        if (level) {
            settings.modelLevels[ref] = level;
        } else {
            delete settings.modelLevels[ref];
        }
        renderComposerLevelRow(ref);
        renderComposerModel();
        await saveComposerSettings();
        pendingModelRef = "";
        closeComposerModelPicker();
    }

    async function saveComposerSettings() {
        var next = JSON.parse(JSON.stringify(settings));
        try {
            var result = await invokeFn("save_settings", { settings: next });
            settingsView = result && result.view ? result.view : settingsView;
            settings = settingsView && settingsView.settings ? settingsView.settings : next;
        } catch (err) {
            toast("保存失败：" + (err && err.message ? err.message : String(err)));
        }
    }
    function closeComposerModelPicker() {
        var modal = document.getElementById("composer-model-modal");
        if (modal) {
            modal.hidden = true;
        }
    }

    async function chooseComposerModel(ref) {
        if (!settings) {
            return;
        }
        settings.defaultChat = ref;
        refreshDefaultChatSelect();
        renderComposerModel();
        // 该模型支持推理等级时：先选等级再关闭，避免用户点不到
        if (modelLevelsOf(ref).length) {
            pendingModelRef = ref;
            renderComposerLevelRow(ref);
            renderComposerModel();
            await saveComposerSettings();
            return;
        }
        closeComposerModelPicker();
        await saveComposerSettings();
    }
    /// 聊天框右侧显示当前使用的模型（外部模型优先，否则本地协调模型）
    function bindComposerModelPicker() {
        var btn = document.getElementById("composer-model-btn");
        if (btn) {
            btn.addEventListener("click", openComposerModelPicker);
        }
        ["composer-model-close", "composer-model-cancel"].forEach(function (id) {
            var node = document.getElementById(id);
            if (node) {
                node.addEventListener("click", closeComposerModelPicker);
            }
        });
    }
    function renderComposerModel() {
        var label = document.getElementById("composer-model-label");
        if (!label) {
            return;
        }
        var ref = roleModelRef();
        var text = modelNameOnly(ref);
        var level = currentModelLevel(ref);
        if (level) {
            text += " · " + level;
        }
        label.textContent = text;
        var btn = document.getElementById("composer-model-btn");
        if (btn) {
            btn.title = "当前角色使用的模型：" + text + "（点击切换）";
        }
    }
    function noteRouteUsage(route) {
        if (!route) {
            return;
        }
        if (route.route === "cloud") {
            cloudEscalations += 1;
        } else if (route.needsCloudConsent) {
            toast("该任务偏认知复杂（" + (route.capability || "unknown") + "），" +
                "本地可先跑；如需更好质量，可关闭左下角「仅本地」申请云端协助");
        }
        updateStatus();
    }

    /// 只告知、不提供选择的提示框（用于阻止操作）
    function uiAlert(message, title) {
        return new Promise(function (resolve) {
            var modal = document.getElementById("app-confirm-modal");
            var text = document.getElementById("app-confirm-text");
            var titleEl = document.getElementById("app-confirm-title");
            var ok = document.getElementById("app-confirm-ok");
            var cancel = document.getElementById("app-confirm-cancel");
            if (!modal || !text || !ok || !cancel) {
                resolve(true);
                return;
            }
            if (titleEl) {
                titleEl.textContent = title || "提示";
            }
            text.textContent = message;
            cancel.hidden = true;
            ok.textContent = "知道了";
            modal.hidden = false;
            function finish() {
                modal.hidden = true;
                cancel.hidden = false;
                ok.textContent = "确定";
                ok.removeEventListener("click", finish);
                resolve(true);
            }
            ok.addEventListener("click", finish);
        });
    }
    /// 应用内确认框（WebView2 里 window.confirm 可能被静默忽略）
    function uiConfirm(message, title) {
        return new Promise(function (resolve) {
            var modal = document.getElementById("app-confirm-modal");
            var text = document.getElementById("app-confirm-text");
            var titleEl = document.getElementById("app-confirm-title");
            var ok = document.getElementById("app-confirm-ok");
            var cancel = document.getElementById("app-confirm-cancel");
            if (!modal || !text || !ok || !cancel) {
                resolve(false);
                return;
            }
            if (titleEl) {
                titleEl.textContent = title || "确认";
            }
            text.textContent = message;
            modal.hidden = false;

            function finish(value) {
                modal.hidden = true;
                ok.removeEventListener("click", onOk);
                cancel.removeEventListener("click", onCancel);
                resolve(value);
            }
            function onOk() { finish(true); }
            function onCancel() { finish(false); }
            ok.addEventListener("click", onOk);
            cancel.addEventListener("click", onCancel);
        });
    }
    async function toggleLocalOnly() {
        if (!invokeFn || !settings) {
            toast("设置不可用，无法切换模式");
            return;
        }
        var next = !autoEscalateOn(); // true = 关闭「仅本地」，切到自动模式
        uiLog("[toggle] click autoEscalate=" + (autoEscalateOn() ? "true" : "false") +
            " next=" + (next ? "true" : "false") +
            " defaultChat=" + ((settings && settings.defaultChat) || "") +
            " label=" + (document.getElementById("composer-model-label") || {}).textContent);
        if (next) {
            var confirmed = await uiConfirm(
                "关闭「仅本地」后：\n\n" +
                "· 认知复杂的任务（长文档高质量总结、跨文件重构、架构分析、开放式多步规划）会直接使用云端，不再逐次询问；\n" +
                "· 敏感内容（secrets.json / .env / .ssh / 密钥文件）与疑似密钥仍受红线保护，绝不上云；\n" +
                "· 每次出网都会写入审计日志；会话云端预算用尽后自动降级为仅本地。\n\n" +
                "");
            if (!confirmed) {
                return;
            }
        }
        // 切回「仅本地」时，若当前选的是云端模型，先确认并说明后果
        if (!next) {
            var ref = (settings.defaultChat || "local");
            uiLog("[toggle] check ref=" + ref);
            if (ref !== "local") {
                var label = modelNameOnly(ref);
                await uiAlert(
                    "当前使用的是云端模型「" + label + "」，不能切到仅本地。\n\n" +
                    "请先在底部模型选择器里改成本地模型，再切换模式。",
                    "无法切到仅本地");
                uiLog("[toggle] blocked: cloud model in use ref=" + ref);
                return;
            }
        }
        settings.cloud = settings.cloud || {};
        settings.cloud.autoEscalate = next;
        renderLocalOnlyToggle();
        try {
            var result = await invokeFn("save_settings", { settings: settings });
            settingsView = result && result.view ? result.view : settingsView;
            settings = settingsView && settingsView.settings ? settingsView.settings : settings;
        } catch (err) {
            settings.cloud.autoEscalate = !next;
            toast("切换失败：" + (err && err.message ? err.message : String(err)));
        }
        renderLocalOnlyToggle();
        updateStatus();
        if (autoEscalateOn() === next) {
            toast(next ? "已切到自动模式：复杂任务将直接使用云端" : "已切回仅本地");
        }
    }

    // ---- 决策 33：云端预算 / 能力档位 / 出网记录 ----
    function renderCloudBudgetInputs() {
        if (!settings || !settings.cloud) {
            return;
        }
        if (els.settingsCloudProfile) {
            els.settingsCloudProfile.value = settings.cloud.profile || "small";
        }
    }

    async function loadCloudAudit() {
        if (!els.cloudAuditList) {
            return;
        }
        els.cloudAuditList.textContent = "";
        if (!invokeFn) {
            els.cloudAuditList.textContent = "浏览器预览：无审计数据";
            return;
        }
        try {
            var records = await invokeFn("cloud_audit", { limit: 20 });
            if (!records || !records.length) {
                els.cloudAuditList.textContent = "暂无出网记录";
                return;
            }
            records.forEach(function (record) {
                var row = document.createElement("div");
                row.className = "settings-row";
                var left = document.createElement("div");
                var title = document.createElement("strong");
                var sent = record.action === "sent";
                title.textContent = (sent ? "已发送" : "已拦截") + " · " + (record.provider || "-");
                var hint = document.createElement("span");
                hint.className = "settings-hint";
                var when = record.ts ? new Date(record.ts).toLocaleString() : "-";
                var detail = when + " · 约 " + (record.estimatedTokens || 0) + " tokens";
                if (record.reason) {
                    detail += " · " + record.reason;
                }
                hint.textContent = detail;
                left.appendChild(title);
                left.appendChild(hint);
                row.appendChild(left);
                els.cloudAuditList.appendChild(row);
            });
        } catch (err) {
            els.cloudAuditList.textContent = "读取失败：" + (err && err.message ? err.message : String(err));
        }
    }

    // ---- 5A：数据目录迁移 ----
    /// 选择新的数据目录（系统原生的文件夹选择框）
    async function pickDataDir() {
        if (!invokeFn) {
            toast("浏览器预览：无法选择目录");
            return;
        }
        try {
            var picked = await invokeFn("pick_data_dir");
            if (!picked) {
                return;
            }
            pendingDataRoot = picked;
            renderPendingDataRoot();
        } catch (err) {
            toast("选择目录失败：" + (err && err.message ? err.message : String(err)));
        }
    }

    function renderPendingDataRoot() {
        if (els.settingsDataPendingRow) {
            els.settingsDataPendingRow.hidden = !pendingDataRoot;
        }
        if (els.settingsDataPending) {
            els.settingsDataPending.textContent = pendingDataRoot
                ? "待切换到：" + pendingDataRoot + "（尚未生效）"
                : "";
            els.settingsDataPending.title = pendingDataRoot || "";
        }
    }

    function cancelPendingDataRoot() {
        if (dataSwitchBusy) {
            toast("正在切换数据目录，请稍候");
            return;
        }
        pendingDataRoot = "";
        renderPendingDataRoot();
        setDataStatus("");
    }

    /// 保存并切换到新数据目录：先问是否迁移，再二次确认
    async function saveDataRoot() {
        if (!invokeFn) {
            toast("浏览器预览：无法切换目录");
            return;
        }
        if (dataSwitchBusy) {
            toast("正在切换数据目录，请稍候");
            return;
        }
        if (!pendingDataRoot) {
            return;
        }
        var migrate = window.confirm(
            "切换到：\n" + pendingDataRoot + "\n\n" +
            "是否把现有数据迁移过去？\n\n" +
            "· 确定 = 迁移（重启后清空旧目录内容，保留文件夹）\n" +
            "· 取消 = 不迁移（新目录从空白开始，旧目录不动）");
        var note = migrate
            ? "迁移成功后，重启应用时会清空旧目录内容（文件夹保留）。"
            : "新目录将从空白开始，旧目录保持不变。";
        if (!window.confirm("确认切换？\n\n" + note + "\n\n切换后需要重启应用才生效。")) {
            return;
        }
        dataSwitchBusy = true;
        if (els.settingsDataSave) {
            els.settingsDataSave.textContent = "切换中…";
        }
        setDataStatus(migrate ? "迁移中…" : "切换中…", "");
        try {
            var report = await invokeFn(migrate ? "migrate_data_root" : "set_data_root", {
                path: pendingDataRoot
            });
            var sizeKb = Math.round((report.bytes || 0) / 1024);
            var text = migrate
                ? "已复制 " + report.files + " 个文件（" + sizeKb + " KB）到新目录"
                : "已切换到新目录";
            text += "。关闭应用后重新打开即可生效。";
            setDataStatus(text, "ok");
            var target = pendingDataRoot;
            uiLog("switch done files=" + report.files + " bytes=" + (report.bytes || 0) + " target=" + target);
            if (els.settingsDataName) {
                els.settingsDataName.textContent =
                    "当前工作区：" + baseName(target) + "（关闭后生效）";
                els.settingsDataName.title = target;
            }
            pendingDataRoot = "";
            renderPendingDataRoot();
            switchedTarget = target;
            switchedFiles = report.files;
            try {
                showRestartNotice();
            } catch (noticeErr) {
                uiLog("showRestartNotice failed: " + noticeErr);
            }
        } catch (err) {
            setDataStatus("切换失败：" + (err && err.message ? err.message : String(err)), "error");
        } finally {
            dataSwitchBusy = false;
            if (els.settingsDataSave) {
                els.settingsDataSave.textContent = "保存并切换";
            }
        }
    }

    /// 前端诊断日志（写进 app.log，沙箱里也能查）
    function uiLog(message) {
        try {
            if (invokeFn) {
                invokeFn("ui_log", { message: String(message) });
            }
        } catch (err) {
            /* 诊断日志失败不影响主流程 */
        }
    }

    /// 行内兜底行（仅在弹窗不可见时启用）
    function renderSwitchedRow(target, text) {
        var row = document.getElementById("settings-data-switched-row");
        var label = document.getElementById("settings-data-switched");
        if (!row) {
            return;
        }
        if (!target) {
            row.hidden = true;
            restartFallbackActive = false;
            return;
        }
        row.hidden = false;
        if (label) {
            label.textContent = text || "";
            label.title = target;
        }
    }

    function switchedText() {
        if (!switchedTarget) {
            return "";
        }
        var suffix = restartNoticeRemaining > 0
            ? "· " + restartNoticeRemaining + " 秒后自动关闭"
            : "· 正在关闭…";
        return "已切换到：" + switchedTarget + "（已复制 " + switchedFiles +
            " 个文件）" + suffix;
    }

    /// 浮层元素用 getElementById 直接取，避免 els 漏赋值时静默失效
    function restartNoticeDom() {
        return {
            box: document.getElementById("restart-notice"),
            text: document.getElementById("restart-notice-text"),
            close: document.getElementById("restart-now"),
            abort: document.getElementById("restart-abort")
        };
    }

    /// 复制完成后：5 秒倒计时自动关闭，也可立即关闭或撤销切换
    function showRestartNotice() {
        var dom = restartNoticeDom();
        restartFallbackActive = false;
        renderSwitchedRow(null);
        if (!dom.box) {
            uiLog("restart notice missing element, fallback to inline row");
            restartFallbackActive = true;
            renderSwitchedRow(switchedTarget, switchedText());
            startRestartCountdown();
            return;
        }
        restartNoticeRemaining = 5;
        dom.box.hidden = false;
        uiLog("restart notice: box=" + Boolean(dom.box) + " text=" + Boolean(dom.text));
        if (dom.close) {
            dom.close.disabled = false;
            dom.close.textContent = "立即关闭";
        }
        renderRestartNotice();
        startRestartCountdown();

        // 客观自检：弹窗虽然显示，但可能被更高层级的面板盖住
        window.setTimeout(function () {
            if (restartFallbackActive) {
                return;
            }
            var box = restartNoticeDom().box;
            if (!box || box.hidden) {
                return;
            }
            var rect = box.getBoundingClientRect();
            var hit = rect.width > 10 && rect.height > 10
                ? document.elementFromPoint(rect.left + rect.width / 2, rect.top + rect.height / 2)
                : null;
            var visible = Boolean(hit && box.contains(hit));
            uiLog("restart notice visible=" + visible);
            if (!visible) {
                restartFallbackActive = true;
                renderSwitchedRow(switchedTarget, switchedText());
            }
        }, 80);
    }

    function startRestartCountdown() {
        window.clearInterval(restartNoticeTimer);
        restartNoticeTimer = window.setInterval(function () {
            restartNoticeRemaining -= 1;
            if (restartNoticeRemaining <= 0) {
                window.clearInterval(restartNoticeTimer);
                restartNoticeTimer = null;
                doCloseApp();
                return;
            }
            renderRestartNotice();
        }, 1000);
    }

    function renderRestartNotice() {
        var dom = restartNoticeDom();
        var draft = els.input && els.input.value.trim().length > 0;
        if (draft) {
            window.clearInterval(restartNoticeTimer);
            restartNoticeTimer = null;
        }
        if (restartFallbackActive) {
            renderSwitchedRow(
                switchedTarget,
                draft
                    ? "已切换到：" + switchedTarget + "（已复制 " + switchedFiles +
                      " 个文件）· 检测到未发送内容，已暂停自动关闭"
                    : switchedText());
        }
        if (!dom.text) {
            return;
        }
        if (draft) {
            dom.text.textContent =
                "数据已复制到新目录。检测到输入框里还有未发送的内容，已暂停自动关闭，请先处理或点「立即关闭」。";
            return;
        }
        dom.text.textContent =
            "数据已复制到新目录。应用将在 " + restartNoticeRemaining +
            " 秒后自动关闭，请重新打开以生效。";
    }

    function hideRestartNotice() {
        window.clearInterval(restartNoticeTimer);
        restartNoticeTimer = null;
        restartFallbackActive = false;
        renderSwitchedRow(null);
        var dom = restartNoticeDom();
        if (dom.box) {
            dom.box.hidden = true;
        }
    }

    async function doCloseApp() {
        window.clearInterval(restartNoticeTimer);
        restartNoticeTimer = null;
        if (!invokeFn) {
            return;
        }
        try {
            await invokeFn("close_app");
        } catch (err) {
            var dom = restartNoticeDom();
            if (dom.text) {
                dom.text.textContent =
                    "关闭失败：" + (err && err.message ? err.message : String(err)) +
                    "。请手动关闭应用后重新打开，或点「撤销切换」继续使用原目录。";
            }
            if (dom.close) {
                dom.close.disabled = true;
            }
        }
    }

    async function abortDataSwitch() {
        hideRestartNotice();
        renderSwitchedRow(null);
        switchedTarget = "";
        switchedFiles = 0;
        if (!invokeFn) {
            return;
        }
        try {
            var root = await invokeFn("abort_data_switch");
            setDataStatus("已撤销切换，仍使用：" + root, "ok");
            if (els.settingsDataName) {
                els.settingsDataName.textContent = "当前工作区：" + baseName(root);
                els.settingsDataName.title = root;
            }
        } catch (err) {
            setDataStatus("撤销失败：" + (err && err.message ? err.message : String(err)), "error");
        }
    }

    /// 数据目录状态提示：kind = "ok" | "error" | 省略（进行中）
    function setDataStatus(text, kind) {
        setStatusLine(els.settingsDataStatus, text, kind);
    }

    // ---- 5B：首次启动引导（3 步） ----
    function onboardingSteps() {
        return [
            {
                title: "欢迎使用小明同学",
                body: "本地优先的个人 AI 助手：默认只跑本地模型（本地路由模型 + 本地协调模型，可在「设置-模型」里自选），敏感内容不离开这台机器。",
                list: [
                    "「仅本地」开关默认开启，复杂任务需要你放权才会用云端",
                    "secrets.json / .env / .ssh 等敏感内容即使放权也绝不上云",
                    "每次出网都写入审计日志，可在「设置 → 外部模型」查看"
                ]
            },
            {
                title: "检查本地环境",
                body: "正在检查 Ollama 连接与所需模型…",
                list: []
            },
            {
                title: "配置外部模型（可选）",
                body: "需要更强能力时可到「设置 → 外部模型」添加 Provider；不配置也能正常使用本地功能。",
                list: ["复杂任务由本地确定性抽取后上云", "会话云端预算可限制花费"]
            }
        ];
    }

    function renderOnboarding() {
        if (!els.onboarding) {
            return;
        }
        var steps = onboardingSteps();
        var step = steps[onboardingStep] || steps[0];
        els.onboarding.hidden = false;
        els.onboardingTitle.textContent = step.title;
        els.onboardingBody.textContent = step.body;
        els.onboardingList.textContent = "";
        (step.list || []).forEach(function (line) {
            var item = document.createElement("li");
            item.textContent = line;
            els.onboardingList.appendChild(item);
        });
        if (els.onboardingStepLabel) {
            els.onboardingStepLabel.textContent = "第 " + (onboardingStep + 1) + " / " + steps.length + " 步";
        }
        if (els.onboardingNext) {
            els.onboardingNext.textContent = onboardingStep === steps.length - 1 ? "开始使用" : "下一步";
        }
        if (onboardingStep === 1) {
            refreshOnboardingEnv();
        }
    }

    async function refreshOnboardingEnv() {
        if (!invokeFn) {
            if (els.onboardingBody) {
                els.onboardingBody.textContent = "浏览器预览模式：无法检测本地模型。";
            }
            return;
        }
        try {
            var report = await invokeFn("check_environment");
            var reachable = report.ollamaReachable !== undefined ? report.ollamaReachable : report.ollama_reachable;
            var missing = report.missingModels || report.missing_models || [];
            var text = reachable ? "Ollama 连接正常" : "Ollama 未连接";
            text += missing.length ? "；缺少模型：" + missing.join("、") : "；所需模型已就绪";
            if (els.onboardingBody) {
                els.onboardingBody.textContent = text;
            }
        } catch (err) {
            if (els.onboardingBody) {
                els.onboardingBody.textContent = "检测失败：" + (err && err.message ? err.message : String(err));
            }
        }
    }

    async function nextOnboarding() {
        var steps = onboardingSteps();
        if (onboardingStep < steps.length - 1) {
            onboardingStep += 1;
            renderOnboarding();
            return;
        }
        await finishOnboarding(false);
    }

    async function finishOnboarding(skipped) {
        if (els.onboarding) {
            els.onboarding.hidden = true;
        }
        if (!settings) {
            return;
        }
        settings.onboardingCompleted = true;
        if (invokeFn) {
            try {
                var result = await invokeFn("save_settings", { settings: settings });
                settingsView = result && result.view ? result.view : settingsView;
                settings = settingsView && settingsView.settings ? settingsView.settings : settings;
            } catch (err) {
                /* 引导状态保存失败不阻塞使用 */
            }
        }
        toast(skipped ? "已跳过引导，可随时在设置里调整" : "设置完成，开始使用吧");
    }

    // ---- 模型扫描与导入 ----
    /// 云端频次闸门：返回 continue / local / cancel
    var cloudGateResolve = null;

    async function cloudGate(sessionId) {
        if (!invokeFn) {
            return "continue";
        }
        var usage = null;
        try {
            usage = await invokeFn("cloud_usage", { sessionId: sessionId });
        } catch (err) {
            return "continue";
        }
        if (!usage || !usage.blocked || usage.suppressed) {
            return "continue";
        }
        var box = document.getElementById("cloud-gate");
        var text = document.getElementById("cloud-gate-text");
        if (!box || !text) {
            return "continue";
        }
        text.textContent = "本会话 " + usage.windowMinutes + " 分钟内已调用云端 " +
            usage.used + " 次（上限 " + usage.limit + "）。继续会产生费用，内容将发送到云端。";
        box.hidden = false;
        var choice = await new Promise(function (resolve) {
            cloudGateResolve = function (value) {
                box.hidden = true;
                cloudGateResolve = null;
                resolve(value);
            };
        });
        uiLog("cloud gate choice=" + choice);
        if (choice === "never") {
            try {
                await invokeFn("set_cloud_prompt_suppressed", { value: true });
            } catch (err) {
                /* 忽略 */
            }
            return "continue";
        }
        return choice;
    }

    /// 打开本地目录；传入 statusEl 时把结果写在行内，否则用 toast
    async function openLocalPath(kind, statusEl) {
        var show = function (text, kind2) {
            if (statusEl) {
                setStatusLine(statusEl, text, kind2);
            } else if (text) {
                toast(text);
            }
        };
        if (!invokeFn) {
            show("浏览器预览：无法打开目录", "error");
            return;
        }
        try {
            var path = await invokeFn("open_local_path", { kind: kind });
            if (path) {
                show("已打开：" + path, "ok");
            }
        } catch (err) {
            show("打开目录失败：" + (err && err.message ? err.message : String(err)), "error");
        }
    }

    /// 通用行内状态提示：kind = "ok" | "error" | 省略
    function setStatusLine(el, text, kind) {
        if (!el) {
            return;
        }
        el.textContent = text || "";
        el.hidden = !text;
        el.classList.toggle("is-error", kind === "error");
        el.classList.toggle("is-ok", kind === "ok");
    }

    async function scanModels() {
        if (!invokeFn) {
            return;
        }
        if (els.scanModelsBtn) {
            els.scanModelsBtn.disabled = true;
        }
        if (els.modelsScanStatus) {
            els.modelsScanStatus.textContent = "扫描中…";
        }
        try {
            var candidates = await invokeFn("scan_models");
            renderScanList(candidates || []);
            if (els.modelsScanStatus) {
                els.modelsScanStatus.textContent = candidates && candidates.length
                    ? "发现 " + candidates.length + " 个 GGUF 文件"
                    : "模型目录里没有找到 .gguf 文件";
            }
        } catch (err) {
            if (els.modelsScanStatus) {
                els.modelsScanStatus.textContent = "扫描失败：" + (err && err.message ? err.message : String(err));
            }
        } finally {
            if (els.scanModelsBtn) {
                els.scanModelsBtn.disabled = false;
            }
        }
    }

    function renderScanList(candidates) {
        if (!els.modelsScanList) {
            return;
        }
        els.modelsScanList.textContent = "";
        candidates.forEach(function (item) {
            var row = document.createElement("div");
            row.className = "settings-row";
            var left = document.createElement("div");
            var title = document.createElement("strong");
            title.textContent = item.relative + "（" + Math.round(item.sizeMb) + " MB）";
            var hint = document.createElement("span");
            hint.className = "settings-hint";
            var detail = "推断：family=" + item.family + " params=" + item.params +
                " quant=" + (item.quant || "未知") + " → " + item.suggestedName +
                "（" + item.suggestedGrade + "）";
            if (item.shard) {
                detail += " · 分片 " + item.shard + "，需先合并";
            } else if (item.imported) {
                detail += " · 已导入";
            }
            hint.textContent = detail;
            left.appendChild(title);
            left.appendChild(hint);
            row.appendChild(left);

            if (!item.shard) {
                var button = document.createElement("button");
                button.type = "button";
                button.className = item.imported ? "ghost" : "primary";
                button.textContent = item.imported ? "重新导入" : "导入";
                button.addEventListener("click", function () {
                    openModelImport(item);
                });
                row.appendChild(button);
            }
            els.modelsScanList.appendChild(row);
        });
    }

    function openModelImport(candidate) {
        if (!els.modelImportForm) {
            return;
        }
        pendingImport = candidate;
        els.modelImportForm.hidden = false;
        els.modelImportName.value = candidate.suggestedName;
        els.modelImportGrade.value = candidate.suggestedGrade;
        els.modelImportCtx.value = "8192";
        els.modelImportDisplay.value = candidate.suggestedName;
        if (els.modelImportPreview) {
            els.modelImportPreview.textContent =
                "FROM ./" + candidate.relative + "\nPARAMETER num_ctx 8192\n";
        }
        els.modelImportName.focus();
    }

    function closeModelImport() {
        if (!els.modelImportForm) {
            return;
        }
        els.modelImportForm.hidden = true;
        pendingImport = null;
    }

    async function submitModelImport() {
        if (!invokeFn || !pendingImport) {
            return;
        }
        var name = els.modelImportName.value.trim().toLowerCase();
        if (!name) {
            toast("请填写模型名");
            return;
        }
        var ctx = parseInt(els.modelImportCtx.value, 10);
        els.modelImportSubmit.disabled = true;
        if (els.modelsScanStatus) {
            els.modelsScanStatus.textContent = "正在导入 " + name + "（ollama create 可能需要几十秒）…";
        }
        try {
            var result = await invokeFn("import_gguf", {
                request: {
                    relative: pendingImport.relative,
                    modelName: name,
                    grade: els.modelImportGrade.value,
                    numCtx: isNaN(ctx) ? 0 : ctx,
                    displayName: els.modelImportDisplay.value.trim()
                }
            });
            closeModelImport();
            await loadLocalModels();
            await loadSettings();
            renderSettings();
            if (els.modelsScanStatus) {
                els.modelsScanStatus.textContent = "已导入 " + result.modelName +
                    "（" + result.grade + " / " + Math.round(result.sizeGb * 10) / 10 + " GB）" +
                    (result.switchedSelection ? "，已设为该档位模型，重启后生效" : "");
            }
            toast("模型导入成功");
            scanModels();
        } catch (err) {
            var message = err && err.message ? err.message : String(err);
            if (els.modelsScanStatus) {
                els.modelsScanStatus.textContent = "导入失败：" + message;
            }
            toast("导入失败：" + message);
        } finally {
            els.modelImportSubmit.disabled = false;
        }
    }

    /// 复制到剪贴板：优先 Clipboard API，失败回退 execCommand
    async function copyText(text) {
        try {
            if (navigator.clipboard && navigator.clipboard.writeText) {
                await navigator.clipboard.writeText(text);
                return true;
            }
        } catch (err) {
            /* 回退到 execCommand */
        }
        try {
            var area = document.createElement("textarea");
            area.value = text;
            area.style.position = "fixed";
            area.style.opacity = "0";
            document.body.appendChild(area);
            area.select();
            var ok = document.execCommand("copy");
            document.body.removeChild(area);
            return ok;
        } catch (err) {
            return false;
        }
    }

    /// 本地优先角色（健康/心理）在本地回答后，提供一次"用云端再分析"的显式放权
    async function maybeOfferCloudForRole(roleId, sessionId, text, reply) {
        var role = roleById(roleId);
        if (!role || role.cloudPolicyResolved !== "localFirst") {
            return;
        }
        if (!reply || !reply.route || !reply.route.needsCloudConsent) {
            return;
        }
        var providerId = (settings && settings.defaultChat) || "local";
        if (providerId === "local") {
            return; // 未配置外部 Provider：不打扰用户
        }
        var provider = providerById(providerId);
        if (!provider || !provider.enabled) {
            return;
        }
        var confirmed = window.confirm(
            "「" + role.name + "」当前用本地模型回答，准确度有限。\n\n" +
            "是否改用云端模型再分析一次？\n" +
            "内容将发送到「" + providerName(providerId) + "」。");
        if (!confirmed) {
            return;
        }
        var typing = showTyping();
        try {
            var context = {
                role: roleId,
                message: text,
                sessionId: sessionId,
                providerId: providerId,
                includeContext: false
            };
            var cloudReply = await executeExternal(context, typing);
            if (cloudReply) {
                typing.remove();
                applyExternalReply(context, cloudReply);
                await loadSessions(roleId);
                updateStatus();
            }
        } catch (err) {
            typing.remove();
            toast("云端分析失败：" + (err && err.message ? err.message : String(err)));
        }
    }

    function systemTheme() {        if (window.matchMedia && window.matchMedia("(prefers-color-scheme: dark)").matches) {
            return "dark";
        }
        return "light";
    }

    function resolveTheme(theme) {
        if (theme === "dark" || theme === "light") {
            return theme;
        }
        return systemTheme();
    }

    function applyTheme(theme) {
        var next = resolveTheme(theme);
        document.documentElement.dataset.theme = next;
        try {
            localStorage.setItem("ai-assistant-theme", theme);
        } catch (err) {
            /* localStorage may be unavailable under file:// restrictions */
        }
        var label = next === "dark" ? "切换浅色" : "切换深色";
        if (els.themeBtn) {
            els.themeBtn.textContent = label;
        }
        if (els.settingsThemeSelect) {
            els.settingsThemeSelect.value = theme;
        }
    }

    function toggleTheme() {
        var next = document.documentElement.dataset.theme === "dark" ? "light" : "dark";
        applyTheme(next);
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

        if (entry.kind === "assistant") {
            var actions = document.createElement("div");
            actions.className = "message-actions";

            var copy = document.createElement("button");
            copy.type = "button";
            copy.className = "ghost";
            copy.textContent = "复制";
            copy.addEventListener("click", function () {
                copyText(entry.content);
            });

            var regen = document.createElement("button");
            regen.type = "button";
            regen.className = "ghost";
            regen.textContent = "重新生成";
            regen.addEventListener("click", function () {
                regenerate();
            });

            var reanswer = document.createElement("button");
            reanswer.type = "button";
            reanswer.className = "ghost";
            reanswer.textContent = "外部模型重答";
            reanswer.addEventListener("click", function () {
                reanswerExternal();
            });

            actions.appendChild(copy);
            actions.appendChild(regen);
            actions.appendChild(reanswer);
            wrapper.appendChild(actions);
        }

        return wrapper;
    }

    function renderHistory() {
        els.history.innerHTML = "";
        var list = currentMessages();

        if (!list.length) {
            var empty = document.createElement("div");
            empty.className = "empty-state";
            var title = document.createElement("strong");
            title.textContent = roleName(currentRole);
            var hint = document.createElement("div");
            var role = roleById(currentRole);
            hint.textContent = currentSessionId()
                ? ((role && role.emptyHint) || "发一条消息试试。")
                : "点左侧 + 新对话开始。";
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

    function addMessage(roleId, sessionId, kind, content, meta) {
        if (!sessionId) {
            return;
        }
        if (!messagesBySession[sessionId]) {
            messagesBySession[sessionId] = [];
        }
        var entry = { kind: kind, content: content, meta: meta || "" };
        messagesBySession[sessionId].push(entry);

        var session = sessionById(roleId, sessionId);
        if (session) {
            session.messageCount = messagesBySession[sessionId].length;
            session.updatedAt = Date.now();
            if (kind === "user" && (!session.title || session.title === "新对话")) {
                session.title = content.slice(0, 24);
            }
        }

        if (sessionId !== currentSessionId()) {
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

    var ROLE_ITEM_HEIGHT = 40;

    function renderRoleWheel() {
        if (!els.roleWheelList) {
            return;
        }
        els.roleWheelList.innerHTML = "";
        roles.forEach(function (role) {
            var item = document.createElement("button");
            item.type = "button";
            item.className = "role-wheel-item" + (role.id === currentRole ? " active" : "");
            item.dataset.role = role.id;
            item.setAttribute("role", "option");
            item.setAttribute("aria-selected", role.id === currentRole ? "true" : "false");

            var name = document.createElement("span");
            name.className = "role-wheel-name";
            name.textContent = role.name;
            item.appendChild(name);

            if (role.id === currentRole) {
                var tick = document.createElement("span");
                tick.className = "role-wheel-check";
                tick.textContent = "✓";
                item.appendChild(tick);
            }

            item.addEventListener("click", function () {
                switchRole(role.id);
                closeRoleWheel();
            });
            els.roleWheelList.appendChild(item);
        });
        layoutRoleWheel();
        if (roleWheelOpen) {
            centerRoleWheel(false);
        }
    }

    /// 轮盘高度自适应：角色少时不留空，多时最多 240px 并滚动
    function layoutRoleWheel() {
        if (!els.roleWheelList) {
            return;
        }
        var count = Math.max(roles.length, 1);
        var view = Math.min(240, count * ROLE_ITEM_HEIGHT);
        els.roleWheelList.style.maxHeight = view + "px";
        els.roleWheelList.style.paddingTop = "0";
        els.roleWheelList.style.paddingBottom = "0";
    }

    function centerRoleWheel(smooth) {
        if (!els.roleWheelList) {
            return;
        }
        roleWheelSuppress = true;
        els.roleWheelList.scrollTo({
            top: roleIndex() * ROLE_ITEM_HEIGHT,
            behavior: smooth ? "smooth" : "auto"
        });
        window.setTimeout(function () {
            roleWheelSuppress = false;
        }, smooth ? 350 : 60);
    }

    function openRoleWheel() {
        renderRoleWheel();
        els.roleWheel.hidden = false;
        els.roleWheelBtn.setAttribute("aria-expanded", "true");
        roleWheelOpen = true;
        window.setTimeout(function () {
            centerRoleWheel(false);
        }, 0);
    }

    function closeRoleWheel() {
        if (!roleWheelOpen) {
            return;
        }
        roleWheelOpen = false;
        els.roleWheel.hidden = true;
        els.roleWheelBtn.setAttribute("aria-expanded", "false");
    }

    function toggleRoleWheel() {
        if (roleWheelOpen) {
            closeRoleWheel();
        } else {
            openRoleWheel();
        }
    }

    function onRoleWheelScroll() {
        // 滚动只负责浏览，不切换角色；避免聊天记录在悬停/滚动时闪动
        // 切换动作统一交由点击（或键盘方向键）确认
        if (!roleWheelOpen) {
            return;
        }
        window.clearTimeout(roleWheelTimer);
        roleWheelTimer = null;
    }

    function onRoleWheelKeydown(event) {
        if (event.key !== "ArrowDown" && event.key !== "ArrowUp") {
            return;
        }
        event.preventDefault();
        var index = roleIndex() + (event.key === "ArrowDown" ? 1 : -1);
        index = Math.max(0, Math.min(roles.length - 1, index));
        switchRole(roles[index].id);
        centerRoleWheel(true);
    }

    function renderRole() {
        var role = roleById(currentRole);
        els.roleTitle.textContent = role ? role.name : currentRole;
        els.roleDesc.textContent = role ? role.desc : "";
        if (els.roleWheelLabel) {
            els.roleWheelLabel.textContent = role ? role.name : currentRole;
        }
        document.title = "小明同学 · " + roleName(currentRole);
        renderRoleWheel();
    }

    function switchRole(roleId) {
        if (roleId === currentRole || !roleById(roleId)) {
            return;
        }
        currentRole = roleId;
        renderRole();
        renderSessions();
        var sessionId = currentSessionByRole[roleId];
        if (sessionId) {
            loadSessionMessages(roleId, sessionId);
        } else {
            loadSessions(roleId);
        }
        renderHistory();
        updateStatus();
    }

    function mockReply(roleId, text) {
        return "[Mock] " + roleName(roleId) + " 收到：" + text +
            "\n\n这是模拟回复。切换到「真实」模式可调用本地模型。";
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

        var sessionId = currentSessionByRole[roleAtSend];
        if (!sessionId) {
            await createSession(roleAtSend);
            sessionId = currentSessionByRole[roleAtSend];
        }
        if (!sessionId) {
            updateStatus("无法创建会话");
            return;
        }

        els.input.value = "";
        addMessage(roleAtSend, sessionId, "user", text, "你 · " + timeText());
        renderSessions();
        sending = true;
        els.sendBtn.disabled = true;
        els.sendBtn.textContent = "发送中…";
        updateStatus();
        requestRoute(text);

        var typing = showTyping();
        var started = window.performance.now();
        await runSendCore(roleAtSend, sessionId, text, typing, started, false);
        finishSend();
    }

    async function runSendCore(roleAtSend, sessionId, text, typing, started, isRegen) {
        var target = effectiveTarget();
        if (target === "error") {
            typing.remove();
            addMessage(roleAtSend, sessionId, "error",
                "CloudOnly 模式需要配置并选择外部 Provider",
                "小明同学 · " + timeText());
            return;
        }

        try {
            if (target !== "local") {
                var context = {
                    role: roleAtSend,
                    message: text,
                    sessionId: sessionId,
                    providerId: target,
                    includeContext: true
                };
                var reply = await executeExternal(context, typing);
                if (reply) {
                    typing.remove();
                    applyExternalReply(context, reply);
                    await loadSessions(roleAtSend);
                    updateStatus();
                }
                return;
            }

            var content;
            var meta;
            if (mode === "real" && realReady && invokeFn) {
                var reply = await invokeFn("send_message", {
                    role: roleAtSend,
                    content: text,
                    sessionId: sessionId
                });
                content = reply.content;
                meta = "小明同学 · " + reply.model + " · " + reply.elapsedMs + "ms" +
                    toolSummary(reply.toolCalls);
                if (reply.sessionId && reply.sessionId !== sessionId) {
                    sessionId = reply.sessionId;
                    currentSessionByRole[roleAtSend] = sessionId;
                }
                noteRouteUsage(reply.route);
                await maybeOfferCloudForRole(roleAtSend, sessionId, text, reply);
            } else {
                await sleep(350);
                content = mockReply(roleAtSend, text);
                meta = "小明同学 · mock · " + Math.round(window.performance.now() - started) + "ms";
            }

            typing.remove();
            addMessage(roleAtSend, sessionId, "assistant", content, meta);
            await loadSessions(roleAtSend);
            if (invokeFn && mode === "real") {
                suggestHint(sessionId, text, content);
            }
            updateStatus();
        } catch (err) {
            typing.remove();
            var message = err && err.message ? err.message : String(err);

            if (target !== "local" && mode === "real" && realReady && invokeFn &&
                settings && settings.cloud && settings.cloud.mode !== "cloudOnly") {
                toast("外部模型失败，已回退本地");
                try {
                    var localReply = await invokeFn("send_message", {
                        role: roleAtSend,
                        content: text,
                        sessionId: sessionId
                    });
                    addMessage(roleAtSend, sessionId, "assistant", localReply.content,
                        "小明同学 · 本地回退 · " + localReply.model + " · " + localReply.elapsedMs + "ms" +
                        toolSummary(localReply.toolCalls));
                    if (localReply.sessionId && localReply.sessionId !== sessionId) {
                        currentSessionByRole[roleAtSend] = localReply.sessionId;
                    }
                    await loadSessions(roleAtSend);
                    updateStatus();
                    return;
                } catch (err2) {
                    /* both failed */
                }
            }

            addMessage(roleAtSend, sessionId, "error", "调用失败：" + message, "小明同学 · " + timeText());
            updateStatus("调用失败");

            if (target === "local" && invokeFn && mode === "real" && availableProviders().length) {
                showProviderPicker("本地调用失败，是否改用外部 Provider？", {
                    role: roleAtSend,
                    message: text,
                    sessionId: sessionId,
                    includeContext: true
                });
            }
        }
    }

    async function clearCurrent() {
        var roleId = currentRole;
        var sessionId = currentSessionByRole[roleId];
        if (!sessionId) {
            return;
        }
        if (!window.confirm("清空当前对话？")) {
            return;
        }

        messagesBySession[sessionId] = [];
        loadedSessions[sessionId] = true;
        routeSeq += 1;
        routing = false;
        lastRoute = null;
        cloudEscalations = 0;

        if (invokeFn) {
            try {
                await invokeFn("clear_history", { role: roleId, sessionId: sessionId });
            } catch (err) {
                /* keep the UI clear even if persistence cleanup fails */
            }
        }

        var session = sessionById(roleId, sessionId);
        if (session) {
            session.messageCount = 0;
            session.updatedAt = Date.now();
        }
        renderSessions();
        renderHistory();
        updateStatus();
    }

    function setSettingsStatus(text) {
        if (els.settingsStatus) {
            els.settingsStatus.textContent = text || "";
        }
    }

    function showSettingsTab(name) {
        if (name === "available") {
            renderAvailableModels();
        }
        if (name === "usage") {
            renderUsagePanel();
        }
        var tabs = document.querySelectorAll(".settings-tab");
        var panels = document.querySelectorAll(".settings-tab-panel");
        for (var i = 0; i < tabs.length; i += 1) {
            tabs[i].classList.toggle("active", tabs[i].dataset.tab === name);
        }
        for (var j = 0; j < panels.length; j += 1) {
            panels[j].classList.toggle("active", panels[j].dataset.panel === name);
        }
    }

    function fillModelSelect(select, selected, preferredGrade) {
        if (!select) {
            return;
        }
        select.innerHTML = "";
        // 只列已登记（models.json）的对话模型；embedding 与未登记模型一律隐藏
        var chat = modelOptions.filter(function (item) {
            return item.kind !== "embedding" && item.registered;
        });
        if (preferredGrade) {
            var preferred = chat.filter(function (item) {
                return item.grade === preferredGrade;
            });
            var others = chat.filter(function (item) {
                return item.grade !== preferredGrade;
            });
            chat = preferred.concat(others);
        }
        var names = chat.map(function (item) {
            return item.name;
        });
        if (selected && names.indexOf(selected) === -1) {
            names.unshift(selected);
        }
        if (!names.length && selected) {
            names = [selected];
        }
        names.forEach(function (model) {
            var matched = chat.filter(function (item) {
                return item.name === model;
            })[0];
            var option = document.createElement("option");
            option.value = model;
            var hint = "";
            if (matched && matched.grade && matched.grade !== "unknown") {
                hint = " · " + matched.grade;
            } else if (!matched) {
                hint = " · 未安装";
            }
            option.textContent = model + hint;
            select.appendChild(option);
        });
        if (selected) {
            select.value = selected;
        }
    }

    function customRoleStats() {
        var customs = roles.filter(function (role) {
            return !role.builtin;
        });
        var used = {};
        customs.forEach(function (role) {
            var number = parseInt(String(role.id).replace("custom-", ""), 10);
            if (!isNaN(number)) {
                used[number] = true;
            }
        });
        var nextId = null;
        for (var n = 5; n <= 8; n += 1) {
            if (!used[n]) {
                nextId = "custom-" + n;
                break;
            }
        }
        return { count: customs.length, max: 4, nextId: nextId };
    }

    function renderSettingsRoles() {
        if (!els.settingsRoleList) {
            return;
        }
        var stats = customRoleStats();
        if (els.roleAddBtn) {
            var full = stats.count >= stats.max;
            els.roleAddBtn.disabled = full;
            els.roleAddBtn.textContent = full
                ? "+ 已达上限（4/4）"
                : "+ 新建角色（" + stats.count + "/4）";
        }
        els.settingsRoleList.innerHTML = "";
        roles.forEach(function (role) {
            var item = document.createElement("div");
            item.className = "settings-role-item";

            var info = document.createElement("div");
            info.className = "settings-role-info";
            var name = document.createElement("strong");
            name.textContent = role.name + (role.builtin ? " · 内置" : " · 自定义");
            var desc = document.createElement("span");
            desc.textContent = role.desc || role.id;
            info.appendChild(name);
            info.appendChild(desc);

            var tagParts = [];
            if (role.cloudPolicyResolved === "localFirst") {
                tagParts.push("本地优先 · 可放权云端");
            } else if (role.cloudPolicyResolved === "localOnly") {
                tagParts.push("仅本地");
            }
            if (role.memoryScope === "explicitOnly") {
                tagParts.push("不默认记忆");
            }
            if (role.cloudPolicyResolved === "localFirst" || role.cloudPolicyResolved === "localOnly") {
                tagParts.push("本地准确度有限");
            }
            if (tagParts.length) {
                var tags = document.createElement("span");
                tags.className = "settings-hint role-tags";
                tags.textContent = tagParts.join(" · ");
                info.appendChild(tags);
            }

            var actions = document.createElement("div");
            actions.className = "settings-role-actions";
            if (!role.builtin) {
                var edit = document.createElement("button");
                edit.type = "button";
                edit.className = "ghost";
                edit.textContent = "编辑";
                edit.addEventListener("click", function () {
                    openRoleForm(role);
                });

                var remove = document.createElement("button");
                remove.type = "button";
                remove.className = "ghost";
                remove.textContent = "删除";
                remove.addEventListener("click", function () {
                    removeRole(role);
                });

                actions.appendChild(edit);
                actions.appendChild(remove);
            } else {
                var badge = document.createElement("span");
                badge.className = "settings-hint";
                badge.textContent = "不可修改";
                actions.appendChild(badge);
            }

            item.appendChild(info);
            item.appendChild(actions);
            els.settingsRoleList.appendChild(item);
        });
    }

    function renderSettings() {
        if (!settings) {
            return;
        }
        var s = settings;
        var models = s.models || {};
        els.settingsThemeSelect.value = s.theme || "system";
        els.settingsOllamaHost.value = models.ollamaHost || "";
        var dataRoot = settingsView && settingsView.dataRoot ? settingsView.dataRoot : "";
        if (els.settingsDataName) {
            els.settingsDataName.textContent = dataRoot
                ? "当前工作区：" + baseName(dataRoot)
                : "当前工作区：-";
            els.settingsDataName.title = dataRoot;
        }
        fillModelSelect(els.settingsRouter, models.router || "", "");
        fillModelSelect(els.settingsCoordinator, models.coordinator || "", "");
        renderSettingsRoles();
        renderAvailableModels();
        renderComposerModel();
        applyTheme(s.theme || "system");
    }

    async function loadLocalModels() {
        if (!invokeFn) {
            return;
        }
        try {
            var options = await invokeFn("model_options");
            modelOptions = Array.isArray(options) ? options : [];
        } catch (err) {
            modelOptions = [];
        }
        // 兼容旧逻辑：仍保留纯名字列表
        localModels = modelOptions.map(function (item) {
            return item.name;
        });
        if (settings) {
            renderSettings();
        }
    }

    async function loadSettings() {
        if (!invokeFn) {
            setSettingsStatus("浏览器预览：设置不可用");
            return;
        }
        try {
            var view = await invokeFn("get_settings");
            settingsView = view || null;
            settings = view && view.settings ? view.settings : null;
            renderSettings();
            renderLocalOnlyToggle();
            renderCloudBudgetInputs();
            loadCloudAudit();
            if (!onboardingShown && settings && !settings.onboardingCompleted) {
                onboardingShown = true;
                onboardingStep = 0;
                renderOnboarding();
            }
            setSettingsStatus("");
        } catch (err) {
            setSettingsStatus("加载设置失败：" + (err && err.message ? err.message : String(err)));
        }
    }

    async function openSettings(tab) {
        if (!els.settingsPanel) {
            return;
        }
        els.settingsPanel.hidden = false;
        showSettingsTab(tab || "general");
        setSettingsStatus("加载中…");
        await loadLocalModels();
        await loadSettings();
        if (tab === "external") {
            await loadProviders();
        }
    }

    function closeSettings() {
        if (els.settingsPanel) {
            els.settingsPanel.hidden = true;
        }
        setSettingsStatus("");
        closeRoleForm();
    }

    function openRoleForm(role) {
        if (!els.roleForm) {
            return;
        }
        var stats = customRoleStats();
        editingRoleId = role ? role.id : null;
        els.roleForm.hidden = false;
        if (els.roleFormIdHint) {
            els.roleFormIdHint.textContent = role
                ? "ID：" + role.id + "（不可修改）"
                : (stats.nextId
                    ? "ID：" + stats.nextId + "（系统分配）"
                    : "自建角色已满 4/4，请先删除一个再新建");
        }
        els.roleFormName.value = role ? role.name : "";
        els.roleFormDesc.value = role ? role.desc : "";
        els.roleFormPrompt.value = role ? (role.systemPrompt || "") : "";
        lastGenerateError = "";
        setGenerateStatus("");
        els.roleFormName.focus();
    }

    /// 生成状态行（独立一行，不挤压按钮）
    function setGenerateStatus(text, copyable) {
        if (!els.roleFormGenerateStatus) {
            return;
        }
        els.roleFormGenerateStatus.textContent = text || "";
        els.roleFormGenerateStatus.hidden = !text;
        els.roleFormGenerateStatus.classList.toggle("copyable", Boolean(copyable && text));
    }

    function closeRoleForm() {
        if (!els.roleForm) {
            return;
        }
        editingRoleId = null;
        els.roleForm.hidden = true;
        els.roleForm.reset();
        lastGenerateError = "";
        setGenerateStatus("");
    }

    /// 按角色名生成专业提示词（本地协调模型，失败不覆盖已填内容）
    async function generateRolePrompt() {
        if (!invokeFn) {
            return;
        }
        var name = els.roleFormName.value.trim();
        if (!name) {
            setSettingsStatus("请先填写角色名称");
            return;
        }
        els.roleFormGenerate.disabled = true;
        setGenerateStatus("生成中（本地模型，约几秒）…", false);
        try {
            var result = await invokeFn("generate_role_prompt", {
                name: name,
                desc: els.roleFormDesc.value.trim() || null
            });
            els.roleFormPrompt.value = result.systemPrompt || "";
            var guardCount = result.guardrails ? result.guardrails.length : 0;
            setGenerateStatus(
                guardCount
                    ? "已生成（含 " + guardCount + " 条护栏），可直接修改"
                    : "已生成，可直接修改",
                false);
        } catch (err) {
            lastGenerateError = err && err.message ? err.message : String(err);
            setGenerateStatus("生成失败：" + lastGenerateError + "（点击复制）", true);
        } finally {
            els.roleFormGenerate.disabled = false;
        }
    }

    async function submitRoleForm(event) {
        event.preventDefault();
        if (!invokeFn) {
            setSettingsStatus("浏览器预览：无法保存角色");
            return;
        }
        var role = {
            id: editingRoleId || "",
            name: els.roleFormName.value.trim(),
            desc: els.roleFormDesc.value.trim(),
            systemPrompt: els.roleFormPrompt.value.trim(),
            modelPreference: "heavy",
            builtin: false,
            enabled: true
        };
        if (!role.name || !role.systemPrompt) {
            setSettingsStatus("角色名称和系统提示词不能为空");
            return;
        }
        try {
            var saved = await invokeFn("save_role", { role: role });
            await loadRoles();
            renderSettingsRoles();
        renderAvailableModels();
        renderComposerModel();
            closeRoleForm();
            setSettingsStatus("角色已保存（ID：" + ((saved && saved.id) || "-") + "）");
        } catch (err) {
            setSettingsStatus("保存角色失败：" + (err && err.message ? err.message : String(err)));
        }
    }

    async function removeRole(role) {
        var confirmed = window.confirm(
            "删除角色「" + role.name + "」？\n\n" +
            "· 该角色的会话、长期记忆、知识库与上下文缓存会被**彻底删除**，无法恢复；\n" +
            "· 释放出来的 ID 会被后续新建角色复用。\n\n确定删除吗？");
        if (!confirmed) {
            return;
        }
        if (!invokeFn) {
            return;
        }
        try {
            await invokeFn("delete_role", { roleId: role.id });
            await loadRoles();
            renderSettingsRoles();
        renderAvailableModels();
        renderComposerModel();
            await loadSessions(currentRole);
            setSettingsStatus("角色已删除");
        } catch (err) {
            setSettingsStatus("删除角色失败：" + (err && err.message ? err.message : String(err)));
        }
    }

    async function saveSettings() {
        if (!invokeFn || !settings) {
            setSettingsStatus("设置不可用");
            return;
        }
        var next = JSON.parse(JSON.stringify(settings));
        next.theme = els.settingsThemeSelect.value || "system";
        next.models = next.models || {};
        next.models.ollamaHost = els.settingsOllamaHost.value.trim() ||
            (settings.models && settings.models.ollamaHost) || "";
        next.models.router = els.settingsRouter.value ||
            (settings.models && settings.models.router) || "";
        next.models.coordinator = els.settingsCoordinator.value ||
            (settings.models && settings.models.coordinator) || "";
        next.defaultChat = els.settingsDefaultChat ? els.settingsDefaultChat.value : "local";
        next.cloud = next.cloud || {};
        next.cloud.mode = els.settingsCloudMode ? els.settingsCloudMode.value : "localFirst";
        next.cloud.enabled = next.cloud.mode === "cloudOnly";
        next.cloud.profile = els.settingsCloudProfile ? els.settingsCloudProfile.value : "small";
        els.settingsSave.disabled = true;
        setSettingsStatus("保存中…");
        try {
            var result = await invokeFn("save_settings", { settings: next });
            settingsView = result && result.view ? result.view : settingsView;
            settings = settingsView && settingsView.settings ? settingsView.settings : next;
            applyTheme(settings.theme);
            renderExternalSettings();
            setSettingsStatus(result && result.restartRequired
                ? "已保存，重启应用后生效"
                : "设置已保存");
        } catch (err) {
            uiLog("[save] error=" + (err && err.message ? err.message : String(err)) + " stack=" + (err && err.stack ? String(err.stack).slice(0, 500) : ""));
            setSettingsStatus("保存失败：" + (err && err.message ? err.message : String(err)));
        } finally {
            els.settingsSave.disabled = false;
        }
    }

    function toast(text, ms) {
        if (!els.toast) {
            return;
        }
        els.toast.textContent = text;
        els.toast.hidden = false;
        window.clearTimeout(toastTimer);
        toastTimer = window.setTimeout(function () {
            els.toast.hidden = true;
        }, ms || 3000);
    }

    function providerById(id) {
        for (var i = 0; i < providers.length; i += 1) {
            if (providers[i].id === id) {
                return providers[i];
            }
        }
        return null;
    }

    function availableProviders() {
        return providers.filter(function (provider) {
            return provider.enabled && provider.hasApiKey;
        });
    }

    function providerName(id) {
        var provider = providerById(id);
        return provider ? provider.name : id;
    }

    function usageText(usage) {
        if (!usage) {
            return "";
        }
        return " · " + (usage.totalTokens || 0) + " tokens";
    }

    function updateUsageCard(providerId, usage) {
        var provider = providerById(providerId);
        if (provider) {
            lastUsage[providerId] = {
                name: provider.name,
                total: usage ? usage.totalTokens : 0
            };
        }
        if (!els.usageName) {
            return;
        }
        if (!providers.length) {
            els.usageName.textContent = "未配置";
            els.usageValue.textContent = "0 tokens";
            return;
        }
        var keys = Object.keys(lastUsage);
        if (keys.length) {
            var latest = lastUsage[keys[keys.length - 1]];
            els.usageName.textContent = latest.name;
            els.usageValue.textContent = latest.total + " tokens";
        } else {
            els.usageName.textContent = "本地模型";
            els.usageValue.textContent = "0 tokens";
        }
    }

    function effectiveTarget() {
        var s = settings || {};
        var mode = s.cloud && s.cloud.mode;
        var defaultChat = roleModelRef();
        if (mode === "cloudOnly") {
            if (defaultChat === "local") {
                return "error";
            }
            var provider = providerById(providerOfRef(defaultChat));
            return provider && provider.enabled && provider.hasApiKey ? defaultChat : "error";
        }
        if (defaultChat === "local") {
            return "local";
        }
        var external = providerById(providerOfRef(defaultChat));
        return external && external.enabled && external.hasApiKey ? defaultChat : "local";
    }

    function refreshDefaultChatSelect() {
        if (!els.settingsDefaultChat) {
            return;
        }
        var current = els.settingsDefaultChat.value ||
            (settings && settings.defaultChat) || "local";
        els.settingsDefaultChat.innerHTML = "";
        var local = document.createElement("option");
        local.value = "local";
        local.textContent = "本地模型";
        els.settingsDefaultChat.appendChild(local);

        // 两层结构：默认模型指向「供应商 : 模型」
        if (!settings) {
            box.textContent = "加载中…";
            return;
        }
        var catalog = (settings && settings.externalModels) || [];
        catalog.forEach(function (item) {
            var provider = providerById(item.providerId);
            if (!provider || !provider.enabled) {
                return;
            }
            var option = document.createElement("option");
            option.value = item.id;
            option.textContent = provider.name + " · " + (item.displayName || item.modelId);
            els.settingsDefaultChat.appendChild(option);
        });

        var exists = catalog.some(function (item) {
            return item.id === current;
        });
        if (current !== "local" && !exists) {
            current = "local";
        }
        els.settingsDefaultChat.value = current;
    }

    function renderProviderList() {
        if (!els.providerList) {
            return;
        }
        els.providerList.innerHTML = "";
        if (!providers.length) {
            var empty = document.createElement("div");
            empty.className = "settings-hint";
            empty.textContent = "还没有外部 Provider。点击「+ 新建」添加一个 OpenAI 兼容接口。";
            els.providerList.appendChild(empty);
            return;
        }
        providers.forEach(function (provider) {
            var item = document.createElement("div");
            item.className = "provider-item";

            var info = document.createElement("div");
            info.className = "provider-info";
            var name = document.createElement("strong");
            var cleanName = String(provider.name || "").replace(/ · (海外|未实测|国内|推荐|已停用)/g, "");
            name.textContent = cleanName + (provider.enabled ? "" : " · 已停用");
            var presetInfo = null;
            for (var pi = 0; pi < (providerPresets || []).length; pi += 1) {
                var candidateBase = (providerPresets[pi].baseUrl || "").replace(/\/+$/, "");
                if (candidateBase && candidateBase === (provider.baseUrl || "").replace(/\/+$/, "")) {
                    presetInfo = providerPresets[pi];
                    break;
                }
            }
            var verifiedIds = (settings && settings.verifiedProviders) || [];
            if (presetInfo && presetInfo.region === "global") {
                var regionTag = document.createElement("span");
                regionTag.className = "provider-tag";
                regionTag.textContent = "海外";
                info.appendChild(regionTag);
            }
            if (presetInfo && presetInfo.unverified && verifiedIds.indexOf(provider.id) === -1) {
                var unverifiedTag = document.createElement("span");
                unverifiedTag.className = "provider-tag";
                unverifiedTag.textContent = "未实测";
                info.appendChild(unverifiedTag);
            }
            var detail = document.createElement("span");
            detail.textContent = provider.baseUrl + " · " + provider.defaultModel +
                " · " + (provider.hasApiKey ? "已配置 Key" : "未配置 Key");
            info.appendChild(name);
            info.appendChild(detail);

            var badge = document.createElement("span");
            badge.className = "provider-badge" + (provider.reachable === true ? " ok" : provider.reachable === false ? " bad" : "");
            badge.textContent = provider.reachable === true
                ? "可达"
                : provider.reachable === false ? "不可达" : "未检测";
            info.appendChild(badge);

            var actions = document.createElement("div");
            actions.className = "provider-actions";

            var test = document.createElement("button");
            test.type = "button";
            test.className = "ghost";
            test.textContent = "测试";
            test.addEventListener("click", function () {
                testProvider(provider, test);
            });

            var edit = document.createElement("button");
            edit.type = "button";
            edit.className = "ghost";
            edit.textContent = "编辑";
            edit.addEventListener("click", function () {
                openProviderForm(provider);
            });

            var remove = document.createElement("button");
            remove.type = "button";
            remove.className = "ghost";
            remove.textContent = "删除";
            remove.addEventListener("click", function () {
                removeProvider(provider);
            });

            actions.appendChild(test);
            // 已存密钥时提供单独清除入口（保留 Provider 配置）
            if (provider.hasApiKey) {
                var clearKey = document.createElement("button");
                clearKey.type = "button";
                clearKey.className = "ghost";
                clearKey.textContent = "清除密钥";
                clearKey.addEventListener("click", function () {
                    clearProviderKey(provider);
                });
                actions.appendChild(clearKey);
            }
            actions.appendChild(edit);
            actions.appendChild(remove);
            item.appendChild(info);
            item.appendChild(actions);
            els.providerList.appendChild(item);
        });
    }

    function renderExportGrants() {
        if (!els.exportGrantList) {
            return;
        }
        els.exportGrantList.innerHTML = "";
        var grants = (settings && settings.fileExportGrants) || [];
        if (!grants.length) {
            var empty = document.createElement("div");
            empty.className = "settings-hint";
            empty.textContent = "暂无文件导出授权。";
            els.exportGrantList.appendChild(empty);
            return;
        }
        var byProvider = {};
        grants.forEach(function (grant) {
            if (!byProvider[grant.providerId]) {
                byProvider[grant.providerId] = [];
            }
            byProvider[grant.providerId].push(grant.roleId);
        });
        Object.keys(byProvider).forEach(function (providerId) {
            var item = document.createElement("div");
            item.className = "provider-item";
            var info = document.createElement("div");
            info.className = "provider-info";
            var name = document.createElement("strong");
            name.textContent = providerName(providerId) || providerId;
            var roles = document.createElement("span");
            roles.textContent = "已授权角色：" + byProvider[providerId].join("、");
            info.appendChild(name);
            info.appendChild(roles);
            var actions = document.createElement("div");
            actions.className = "provider-actions";
            var revoke = document.createElement("button");
            revoke.type = "button";
            revoke.className = "ghost";
            revoke.textContent = "撤销授权";
            revoke.addEventListener("click", function () {
                revokeExport(providerId);
            });
            actions.appendChild(revoke);
            item.appendChild(info);
            item.appendChild(actions);
            els.exportGrantList.appendChild(item);
        });
    }

    function renderExternalSettings() {
        if (!settings || !els.settingsCloudMode) {
            return;
        }
        var mode = settings.cloud && settings.cloud.mode;
        els.settingsCloudMode.value = mode === "cloudOnly" ? "cloudOnly" : "localFirst";
        refreshDefaultChatSelect();
        renderProviderList();
        renderExportGrants();
    }

    async function loadProviders() {
        if (!invokeFn) {
            return;
        }
        try {
            var list = await invokeFn("list_providers");
            providers = Array.isArray(list) ? list : [];
            renderExternalSettings();
            updateUsageCard(null, null);
        } catch (err) {
            providers = [];
        }
    }

    async function loadProviderPresets() {
        if (!els.providerFormPreset || !invokeFn) {
            return;
        }
        try {
            var presets = await invokeFn("provider_presets");
            try {
                var fileVersion = await invokeFn("presets_version");
                var appliedVersion = (settings && settings.presetVersion) || 0;
                if (fileVersion && fileVersion > appliedVersion) {
                    setSettingsStatus("供应商预设已更新（v" + fileVersion + "），可在下拉里查看新供应商");
                }
            } catch (err) {
                /* 版本提示失败不影响预设加载 */
            }
            providerPresets = presets || [];
            var select = els.providerFormPreset;
            select.textContent = "";
            var placeholder = document.createElement("option");
            placeholder.value = "";
            placeholder.textContent = "选择预设…";
            select.appendChild(placeholder);
            (presets || []).forEach(function (preset) {
                var group = select.querySelector('optgroup[data-group="' + preset.group + '"]');
                if (!group) {
                    group = document.createElement("optgroup");
                    group.dataset.group = preset.group;
                    group.label = preset.group === "official"
                    ? "直连（官方）"
                    : preset.group === "custom"
                        ? "自定义"
                        : "中间代理";
                    select.appendChild(group);
                }
                var option = document.createElement("option");
                option.value = preset.id;

                option.textContent = preset.name;
                option.dataset.baseUrl = preset.baseUrl || "";
                option.dataset.note = preset.note || "";
                group.appendChild(option);
            });
        } catch (err) {
            /* 预设加载失败不阻塞手动填写 */
        }
    }

    function applyProviderPreset() {
        if (!els.providerFormPreset) {
            return;
        }
        var options = els.providerFormPreset.selectedOptions;
        var option = options && options[0];
        if (!option || !option.value) {
            return;
        }
        if (option.dataset.baseUrl) {
            els.providerFormBase.value = option.dataset.baseUrl;
        }
        // 预设自带模型清单：直接填好，避免手打模型名写错
        var preset = null;
        for (var i = 0; i < providerPresets.length; i += 1) {
            if (providerPresets[i].id === option.value) {
                preset = providerPresets[i];
                break;
            }
        }
        if (preset) {
            // 模型清单一律靠「连接」实时拉取，预设里不写死
            els.providerFormModels.value = "";
            els.providerFormDefault.value = "";
            providerDisplayNames = {};
            renderProviderModelRows();
            setProviderStatus("", "");
            var endpoints = preset.endpoints || [];
            if (endpoints.length && endpoints[0].baseUrl) {
                els.providerFormBase.value = endpoints[0].baseUrl;
            }
            if (els.providerModelsStatus) {
                var extra = endpoints.length > 1
                    ? "；可选端点：" + endpoints.map(function (item) { return item.label; }).join(" / ")
                    : "";
                els.providerModelsStatus.textContent = (preset.note || "") + extra;
            }
        }
        if (!els.providerFormId.value.trim() && option.value !== "custom") {
            els.providerFormId.value = option.value;
        }
        if (!els.providerFormName.value.trim()) {
            els.providerFormName.value = (preset && preset.name) ? preset.name : option.textContent;
        }
        if (els.providerModelsStatus) {
            els.providerModelsStatus.textContent = option.dataset.note || "";
        }
    }

    function renderModelPicker(models) {
        if (!els.providerModelsPicker) {
            return;
        }
        var selected = els.providerFormModels.value.split(",").map(function (item) {
            return item.trim();
        }).filter(Boolean);
        els.providerModelsPicker.textContent = "";
        els.providerModelsPicker.hidden = false;
        models.forEach(function (model) {
            var label = document.createElement("label");
            label.className = "provider-check";
            var box = document.createElement("input");
            box.type = "checkbox";
            box.value = model;
            box.checked = selected.indexOf(model) !== -1;
            label.appendChild(box);
            label.appendChild(document.createTextNode(" " + model));
            els.providerModelsPicker.appendChild(label);
        });
        var actions = document.createElement("div");
        actions.className = "role-form-actions";
        var apply = document.createElement("button");
        apply.type = "button";
        apply.className = "primary";
        apply.textContent = "应用所选";
        apply.addEventListener("click", function () {
            var picked = Array.prototype.slice
                .call(els.providerModelsPicker.querySelectorAll('input[type="checkbox"]'))
                .filter(function (box) {
                    return box.checked;
                })
                .map(function (box) {
                    return box.value;
                });
            els.providerFormModels.value = picked.join(", ");
            if (!els.providerFormDefault.value.trim() && picked.length) {
                els.providerFormDefault.value = picked[0];
            }
            els.providerModelsPicker.hidden = true;
            if (els.providerModelsStatus) {
                els.providerModelsStatus.textContent = "已应用 " + picked.length + " 个模型";
            }
        });
        actions.appendChild(apply);
        els.providerModelsPicker.appendChild(actions);
    }

    /// 测试连接：只报告成功/失败，不渲染模型
    async function testProviderConnection() {
        if (!els.providerFormBase.value.trim()) {
            toast("请先选择供应商或填写 Base URL");
            return;
        }
        setProviderStatus("测试中…", "");
        try {
            var models = await invokeFn("fetch_provider_models", {
                baseUrl: els.providerFormBase.value.trim(),
                apiKey: els.providerFormKey.value.trim() || null,
                providerId: els.providerFormId.value.trim() || null
            });
            setProviderStatus("连接成功，可用模型 " + models.length + " 个", "ok");
        } catch (err) {
            setProviderStatus("连接失败：" + (err && err.message ? err.message : String(err)), "error");
        }
    }

    var pickerModels = [];

    /// 打开选模型弹窗
    async function openModelPicker() {
        if (!els.providerFormBase.value.trim()) {
            toast("请先选择供应商或填写 Base URL");
            return;
        }
        setProviderStatus("获取模型中…", "");
        try {
            pickerModels = await invokeFn("fetch_provider_models", {
                baseUrl: els.providerFormBase.value.trim(),
                apiKey: els.providerFormKey.value.trim() || null,
                providerId: els.providerFormId.value.trim() || null
            });
        } catch (err) {
            setProviderStatus("获取失败：" + (err && err.message ? err.message : String(err)), "error");
            return;
        }
        if (!pickerModels.length) {
            setProviderStatus("该接口没有返回模型，可用「+ 添加模型」手动填写", "error");
            return;
        }
        setProviderStatus("已获取 " + pickerModels.length + " 个模型", "ok");
        var search = document.getElementById("provider-picker-search");
        if (search) {
            search.value = "";
        }
        renderPickerList("");
        var modal = document.getElementById("provider-picker-modal");
        if (modal) {
            modal.hidden = false;
        }
    }

    function renderPickerList(keyword) {
        var list = document.getElementById("provider-picker-list");
        if (!list) {
            return;
        }
        list.textContent = "";
        var key = (keyword || "").trim().toLowerCase();
        var checked = 0;
        pickerModels.forEach(function (id) {
            if (key && id.toLowerCase().indexOf(key) === -1) {
                return;
            }
            var row = document.createElement("label");
            row.className = "provider-picker-item";
            var box = document.createElement("input");
            box.type = "checkbox";
            box.checked = true;
            box.value = id;
            box.addEventListener("change", updatePickerCount);
            var text = document.createElement("span");
            text.textContent = id;
            row.appendChild(box);
            row.appendChild(text);
            list.appendChild(row);
            if (box.checked) {
                checked += 1;
            }
        });
        updatePickerCount();
    }

    function updatePickerCount() {
        var list = document.getElementById("provider-picker-list");
        var count = document.getElementById("provider-picker-count");
        if (!list || !count) {
            return;
        }
        var all = list.querySelectorAll('input[type="checkbox"]');
        var checked = list.querySelectorAll('input[type="checkbox"]:checked');
        count.textContent = "已选 " + checked.length + " / " + all.length;
    }

    function closeModelPicker() {
        var modal = document.getElementById("provider-picker-modal");
        if (modal) {
            modal.hidden = true;
        }
    }

    function applyPickedModels() {
        var list = document.getElementById("provider-picker-list");
        if (!list) {
            return;
        }
        var picked = [];
        Array.prototype.forEach.call(list.querySelectorAll('input[type="checkbox"]:checked'), function (box) {
            picked.push(box.value);
        });
        if (!picked.length) {
            toast("没有勾选任何模型");
            return;
        }
        var merged = parseModelIds();
        picked.forEach(function (id) {
            if (merged.indexOf(id) === -1) {
                merged.push(id);
            }
        });
        els.providerFormModels.value = merged.join(", ");
        renderProviderModelRows();
        closeModelPicker();
    }

    function setProviderStatus(text, kind) {
        var el = document.getElementById("provider-models-status");
        if (!el) {
            return;
        }
        el.textContent = text || "";
        el.hidden = !text;
        el.classList.toggle("is-ok", kind === "ok");
        el.classList.toggle("is-error", kind === "error");
    }
    function openAddModelModal() {
        var modal = document.getElementById("provider-add-modal");
        var idInput = document.getElementById("provider-add-id");
        var nameInput = document.getElementById("provider-add-name");
        if (idInput) { idInput.value = ""; }
        if (nameInput) { nameInput.value = ""; }
        if (modal) { modal.hidden = false; }
        if (idInput) { idInput.focus(); }
    }

    function closeAddModelModal() {
        var modal = document.getElementById("provider-add-modal");
        if (modal) { modal.hidden = true; }
    }
    /// 把模型目录（含显示名）写入 settings.externalModels
    async function persistProviderModels(providerId) {
        if (!settings || !providerId) {
            return;
        }
        var ids = parseModelIds();
        var list = (settings.externalModels || []).filter(function (item) {
            return item.providerId !== providerId;
        });
        ids.forEach(function (modelId) {
            list.push({
                id: providerId + ":" + modelId,
                providerId: providerId,
                modelId: modelId,
                displayName: providerDisplayNames[modelId] || "",
                verified: true
            });
        });
        var next = JSON.parse(JSON.stringify(settings));
        next.externalModels = list;
        try {
            var result = await invokeFn("save_settings", { settings: next });
            settingsView = result && result.view ? result.view : settingsView;
            settings = settingsView && settingsView.settings ? settingsView.settings : next;
            refreshDefaultChatSelect();
        } catch (err) {
            setSettingsStatus("模型目录保存失败：" + (err && err.message ? err.message : String(err)));
        }
    }
    /// 费用页：概览聚合 + 供应商额度卡
    async function renderUsagePanel() {
        function money(value) {
            return "$" + Number(value || 0).toFixed(4);
        }
        function detail(summary) {
            return "调用 " + (summary.requests || 0) + " 次 · 输入 " +
                Math.round((summary.tokensIn || 0) / 1000) + "k / 输出 " +
                Math.round((summary.tokensOut || 0) / 1000) + "k";
        }
        var periods = [
            { period: "today", cost: "usage-today-cost", detail: "usage-today-detail" },
            { period: "month", cost: "usage-month-cost", detail: "usage-month-detail" },
            { period: "total", cost: "usage-total-cost", detail: "usage-total-detail" }
        ];
        for (var i = 0; i < periods.length; i += 1) {
            var item = periods[i];
            try {
                var summary = await invokeFn("usage_summary", { period: item.period });
                var costEl = document.getElementById(item.cost);
                var detailEl = document.getElementById(item.detail);
                if (costEl) {
                    costEl.textContent = money(summary && summary.cost);
                }
                if (detailEl) {
                    detailEl.textContent = detail(summary || {});
                }
            } catch (err) {
                /* 聚合失败保持占位 */
            }
        }
        var updated = document.getElementById("usage-updated");
        if (updated) {
            var now = new Date();
            updated.textContent = "更新于 " + now.getHours().toString().padStart(2, "0") + ":" +
                now.getMinutes().toString().padStart(2, "0");
        }
        renderUsageProviders();
    }

    /// 供应商额度：OpenCode 走 usage 接口渲染三条柱状图；DeepSeek 走余额
    async function fetchProviderQuota(provider, host) {
        host.textContent = "";
        var isGo = provider.id === "opencode-go" || (provider.baseUrl || "").indexOf("opencode.ai") !== -1;
        if (isGo) {
            try {
                var data = await invokeFn("go_usage", { providerId: provider.id });
                var usage = data && data.usage ? data.usage : null;
                if (!usage) {
                    host.textContent = "额度接口未返回数据";
                    return;
                }
                [{ key: "rolling", label: "滚动 5 小时" },
                 { key: "weekly", label: "本周" },
                 { key: "monthly", label: "本月" }].forEach(function (item) {
                    var node = usage[item.key];
                    if (!node || typeof node.percent !== "number") {
                        return;
                    }
                    var row = document.createElement("div");
                    row.className = "quota-row";
                    var label = document.createElement("span");
                    label.className = "quota-label";
                    label.textContent = item.label;
                    var track = document.createElement("span");
                    track.className = "quota-track";
                    var fill = document.createElement("span");
                    var percent = Math.max(0, Math.min(100, node.percent));
                    fill.className = "quota-fill " + (percent > 90 ? "danger" : percent > 70 ? "warn" : "ok");
                    fill.style.width = percent === 0 ? "2px" : percent + "%";
                    track.appendChild(fill);
                    var value = document.createElement("span");
                    value.className = "quota-value";
                    value.textContent = percent + "%";
                    row.appendChild(label);
                    row.appendChild(track);
                    row.appendChild(value);
                    host.appendChild(row);
                });
            } catch (err) {
                var reason = err && err.message ? err.message : String(err);
                uiLog("[usage] go_usage failed provider=" + provider.id + " error=" + reason);
                host.textContent = "额度查询失败：" + reason;
            }
            return;
        }
        try {
            var result = await invokeFn("provider_balance", { providerId: provider.id });
            var balance = result && typeof result[0] === "number" ? result[0] : null;
            if (balance === null) {
                host.textContent = "余额请登录控制台查看";
                return;
            }
            var currency = (result && result[1]) || "";
            host.textContent = "余额 " + (currency === "CNY" ? "¥" : "$") + balance.toFixed(2);
        } catch (err) {
            host.textContent = "余额请登录控制台查看";
        }
    }
    /// 拉取 Go 套餐额度并渲染百分比
    async function fetchGoUsage(providerId, noteEl) {
        uiLog("[usage] go_usage request provider=" + providerId);
        try {
            var data = await invokeFn("go_usage", { providerId: providerId });
            uiLog("[usage] go_usage ok provider=" + providerId + " keys=" + Object.keys(data || {}).join("/"));
            var usage = data && data.usage ? data.usage : null;
            if (!usage) {
                noteEl.textContent = "额度接口未返回数据";
                return;
            }
            function line(label, node) {
                if (!node) {
                    return "";
                }
                var percent = typeof node.percent === "number" ? node.percent : null;
                var reset = node.resetsAt ? String(node.resetsAt).replace("T", " ").replace("Z", "") : "";
                if (percent === null) {
                    return "";
                }
                return label + " " + percent + "%" + (reset ? "（重置 " + reset + "）" : "");
            }
            var parts = [
                line("滚动 5 小时", usage.rolling),
                line("本周", usage.weekly),
                line("本月", usage.monthly)
            ].filter(Boolean);
            noteEl.textContent = parts.length ? parts.join(" · ") : "额度字段缺失";
        } catch (err) {
            var reason = err && err.message ? err.message : String(err);
            uiLog("[usage] go_usage failed provider=" + providerId + " error=" + reason);
            noteEl.textContent = "额度查询失败：" + reason;
        }
    }
    /// 供应商额度卡（本期只列已配置 Key 的供应商）
    async function renderUsageProviders() {
        var box = document.getElementById("usage-providers");
        if (!box) {
            return;
        }
        box.textContent = "";
        // 直接向后端取最新供应商列表，避免依赖前端缓存是否已加载
        var list = [];
        try {
            var fresh = await invokeFn("list_providers");
            list = (fresh || []).filter(function (item) {
                return item.id !== "local";
            });
            uiLog("[usage] fresh providers=" + list.length + " detail=" + list.map(function (item) {
                return item.id + ":" + (item.hasApiKey ? "key" : "nokey");
            }).join(","));
        } catch (err) {
            uiLog("[usage] providers fetch failed error=" + (err && err.message ? err.message : String(err)));
            list = ((settings && settings.providers) || []).filter(function (item) {
                return item.id !== "local";
            });
        }
        if (!list.length) {
            var empty = document.createElement("div");
            empty.className = "usage-provider-note";
            empty.textContent = "还没有云端供应商。请先在「云端供应商」里添加。";
            box.appendChild(empty);
            return;
        }
        list.forEach(function (provider) {
            var card = document.createElement("div");
            card.className = "usage-provider-card";
            var head = document.createElement("div");
            head.className = "usage-provider-head";
            var dot = document.createElement("span");
            dot.className = "status-dot " + (provider.hasApiKey ? "ok" : "unknown");
            var name = document.createElement("strong");
            name.textContent = provider.name;
            head.appendChild(dot);
            head.appendChild(name);
            card.appendChild(head);
            var show = document.createElement("label");
            show.className = "provider-check";
            var showBox = document.createElement("input");
            showBox.type = "checkbox";
            showBox.checked = ((settings && settings.usageProviders) || []).indexOf(provider.id) !== -1;
            showBox.addEventListener("change", function () {
                settings.usageProviders = settings.usageProviders || [];
                var index = settings.usageProviders.indexOf(provider.id);
                if (showBox.checked && index === -1) {
                    settings.usageProviders.push(provider.id);
                } else if (!showBox.checked && index !== -1) {
                    settings.usageProviders.splice(index, 1);
                }
                saveComposerSettings();
                refreshUsageCard();
            });
            show.appendChild(showBox);
            show.appendChild(document.createTextNode(" 显示在主界面"));
            head.appendChild(show);
            card.appendChild(head);

            var body = document.createElement("div");
            body.className = "usage-provider-note";
            body.textContent = provider.hasApiKey ? "额度查询中…" : "未配置 API Key";
            card.appendChild(body);
            if (provider.hasApiKey) {
                fetchProviderQuota(provider, body);
            }
            box.appendChild(card);
        });
    }
    function renderAvailableModels() {
        try {
            renderAvailableModelsInner();
        } catch (err) {
            var box = document.getElementById("available-model-list");
            if (box) {
                box.textContent = "可用模型渲染失败：" + (err && err.message ? err.message : String(err));
            }
        }
    }

    /// 固定展示的"本地模型"卡片：只读，来源是设置-模型里的路由/协调模型
    function renderLocalModelCard(box) {
        var router = settings && settings.models ? settings.models.router : "";
        var coordinator = settings && settings.models ? settings.models.coordinator : "";
        if (!router && !coordinator) {
            return;
        }
        var card = document.createElement("div");
        card.className = "available-card";
        var head = document.createElement("div");
        head.className = "available-card-head";
        var dot = document.createElement("span");
        dot.className = "status-dot ok";
        var title = document.createElement("strong");
        title.textContent = "本地（Ollama）";
        var badge = document.createElement("span");
        badge.className = "available-count-badge";
        badge.textContent = "固定";
        head.appendChild(dot);
        head.appendChild(title);
        head.appendChild(badge);
        card.appendChild(head);

        var row = document.createElement("div");
        row.className = "available-model-row enabled";
        var rowDot = document.createElement("span");
        rowDot.className = "status-dot ok";
        var block = document.createElement("div");
        block.className = "model-name-block";
        var name = document.createElement("span");
        name.className = "name";
        name.textContent = coordinator || router;
        var id = document.createElement("span");
        id.className = "id";
        id.textContent = "协调 " + (coordinator || "-") + " · 路由 " + (router || "-");
        block.appendChild(name);
        block.appendChild(id);
        row.appendChild(rowDot);
        row.appendChild(block);
        card.appendChild(row);
        box.appendChild(card);
    }
    function renderAvailableModelsInner() {
        var box = document.getElementById("available-model-list");
        var summary = document.getElementById("available-summary");
        if (!box) {
            return;
        }
        if (!settings) {
            box.textContent = "加载中…";
            return;
        }
        var catalog = (settings && settings.externalModels) || [];
        var enabled = catalog.filter(function (item) { return item.enabled !== false; });
        if (summary) {
            summary.textContent = "已启用 " + enabled.length + " / " + catalog.length;
        }
        box.textContent = "";
        renderLocalModelCard(box);
        if (!catalog.length) {
            box.textContent = "还没有可用模型。请先到「云端供应商」获取模型。";
            return;
        }
        var groups = {};
        catalog.forEach(function (item) {
            if (!groups[item.providerId]) {
                groups[item.providerId] = [];
            }
            groups[item.providerId].push(item);
        });
        var providerOrder = ((settings && settings.providerOrder) || []).slice();
        if (!providerOrder.length) {
            providerOrder = ((settings && settings.providers) || []).map(function (item) {
                return item.id;
            });
            if (providerOrder.indexOf("local") === -1) {
                providerOrder.unshift("local");
            }
        }
        var groupKeys = Object.keys(groups).sort(function (a, b) {
            var ia = providerOrder.indexOf(a);
            var ib = providerOrder.indexOf(b);
            return (ia < 0 ? 999 : ia) - (ib < 0 ? 999 : ib);
        });
        if (providerOrder.indexOf("local") === -1) {
            providerOrder.unshift("local");
        }
        if (groupKeys.indexOf("local") === -1) {
            groupKeys.push("local");
            groupKeys.sort(function (a, b) {
                var ia = providerOrder.indexOf(a);
                var ib = providerOrder.indexOf(b);
                return (ia < 0 ? 999 : ia) - (ib < 0 ? 999 : ib);
            });
        }
        groupKeys.forEach(function (providerId) {
            if (providerId === "local") {
                return;
            }
            var provider = providerById(providerId);
            var items = groups[providerId].slice();
            var orderIds = ((settings && settings.modelOrder) || {})[providerId] || [];
            if (orderIds.length) {
                items.sort(function (a, b) {
                    var ia = orderIds.indexOf(a.modelId);
                    var ib = orderIds.indexOf(b.modelId);
                    return (ia < 0 ? 9999 : ia) - (ib < 0 ? 9999 : ib);
                });
            } else {
                items.sort(function (a, b) {
                    return (a.sortIndex || 0) - (b.sortIndex || 0);
                });
            }
            var card = document.createElement("div");
            card.className = "available-card";

            var head = document.createElement("div");
            head.className = "available-card-head";
            var okCount = items.filter(function (item) { return item.verified; }).length;
            var failed = items.filter(function (item) { return item.lastError; });
            var dot = document.createElement("span");
            dot.className = "status-dot " + (okCount === items.length ? "ok" : okCount ? "warn" : "unknown");
            var title = document.createElement("strong");
            var rawName = provider ? String(provider.name || "") : providerId;
            var cleanTitle = providerId === "local"
                ? "本地（Ollama）"
                : rawName.replace(/ · (海外|未实测|国内|推荐|已停用)/g, "");
            title.textContent = cleanTitle;
            var badge = document.createElement("span");
            badge.className = "available-count-badge";
            badge.textContent = "可用 " + okCount + "/" + items.length;
            if (failed.length) {
                badge.title = failed.length + " 个失败，原因：" + failed[0].lastError;
            }
            head.appendChild(dot);
            head.appendChild(title);
            head.appendChild(badge);

            var headActions = document.createElement("div");
            headActions.className = "available-head-actions";

            var testBtn = document.createElement("button");
            testBtn.type = "button";
            testBtn.className = "ghost";
            testBtn.textContent = availableTesting[providerId] ? "停止" : "全部测试";
            testBtn.addEventListener("click", function () {
                testProviderModels(providerId, items, testBtn);
            });
            headActions.appendChild(testBtn);

            var upProv = document.createElement("button");
            upProv.type = "button";
            upProv.className = "ghost";
            upProv.textContent = "↑";
            upProv.title = "上移供应商";
            upProv.addEventListener("click", function () {
                moveProvider(providerId, -1);
            });
            var downProv = document.createElement("button");
            downProv.type = "button";
            downProv.className = "ghost";
            downProv.textContent = "↓";
            downProv.title = "下移供应商";
            downProv.addEventListener("click", function () {
                moveProvider(providerId, 1);
            });
            headActions.appendChild(upProv);
            headActions.appendChild(downProv);
            head.appendChild(headActions);
            card.appendChild(head);

            items.forEach(function (item) {
                var row = document.createElement("div");
                row.className = "available-model-row" + (item.enabled !== false ? " enabled" : "");

                var rowDot = document.createElement("span");
                rowDot.className = "status-dot " +
                    (item.verified ? "ok" : item.lastError ? "bad" : "unknown");
                if (item.lastError) {
                    rowDot.title = item.lastError;
                }

                var nameBlock = document.createElement("div");
                nameBlock.className = "model-name-block";
                var nameSpan = document.createElement("span");
                nameSpan.className = "name";
                nameSpan.textContent = item.displayName || item.modelId;
                var idSpan = document.createElement("span");
                idSpan.className = "id";
                idSpan.textContent = item.modelId;
                nameBlock.appendChild(nameSpan);
                nameBlock.appendChild(idSpan);

                var actions = document.createElement("div");
                actions.className = "available-row-actions";
                var testOne = document.createElement("button");
                testOne.type = "button";
                testOne.className = "ghost";
                testOne.textContent = "测试";
                testOne.addEventListener("click", async function () {
                    testOne.disabled = true;
                    testOne.textContent = "测试中";
                    try {
                        await invokeFn("test_external_model", { modelId: item.id });
                    } catch (err) {
                        /* 结果已写回状态 */
                    }
                    await loadSettings();
                    renderAvailableModels();
                });
                actions.appendChild(testOne);
                var up = document.createElement("button");
                up.type = "button";
                up.className = "ghost";
                up.textContent = "↑";
                up.title = "上移";
                up.addEventListener("click", function () {
                    moveAvailableModel(item, -1);
                });
                var down = document.createElement("button");
                down.type = "button";
                down.className = "ghost";
                down.textContent = "↓";
                down.title = "下移";
                down.addEventListener("click", function () {
                    moveAvailableModel(item, 1);
                });
                actions.appendChild(up);
                actions.appendChild(down);

                var toggle = document.createElement("label");
                toggle.className = "provider-check";
                var checkbox = document.createElement("input");
                checkbox.type = "checkbox";
                checkbox.checked = item.enabled !== false;
                checkbox.addEventListener("change", function () {
                    item.enabled = checkbox.checked;
                    persistAvailableModels();
                });
                toggle.appendChild(checkbox);
                toggle.appendChild(document.createTextNode(" 启用"));

                row.appendChild(rowDot);
                row.appendChild(nameBlock);
                row.appendChild(actions);
                row.appendChild(toggle);
                card.appendChild(row);
            });        box.appendChild(card);
        });
    }

    /// 模型上下移动：顺序交给后端保存，避免前端快照被回写覆盖
    async function moveAvailableModel(item, delta) {
        if (!settings) {
            return;
        }
        var providerId = item.providerId;
        var ids = ((settings.modelOrder || {})[providerId] || []).slice();
        if (!ids.length) {
            ids = settings.externalModels
                .filter(function (entry) { return entry.providerId === providerId; })
                .map(function (entry) { return entry.modelId; });
        }
        var index = ids.indexOf(item.modelId);
        var target = index + delta;
        if (index < 0 || target < 0 || target >= ids.length) {
            return;
        }
        ids.splice(index, 1);
        ids.splice(target, 0, item.modelId);
        try {
            await invokeFn("set_model_order", { providerId: providerId, ids: ids });
            await loadSettings();
        } catch (err) {
            setSettingsStatus("保存模型顺序失败：" + (err && err.message ? err.message : String(err)));
        }
        renderAvailableModels();
    }
    async function moveProvider(providerId, delta) {
        if (!settings) {
            return;
        }
        var order = (settings.providerOrder || []).slice();
        if (!order.length) {
            order = (settings.providers || []).map(function (item) { return item.id; });
            if (order.indexOf("local") === -1) {
                order.unshift("local");
            }
        }
        if (providerId === "local") {
            return;
        }
        var index = order.indexOf(providerId);
        var target = index + delta;
        if (index < 0 || target < 0 || target >= order.length) {
            return;
        }
        order.splice(index, 1);
        order.splice(target, 0, providerId);
        uiLog("[order] move " + providerId + " delta=" + delta + " index=" + index + " target=" + target + " -> " + order.join(","));
        try {
            await invokeFn("set_provider_order", { ids: order });
            await loadSettings();
            uiLog("[order] saved, now=" + ((settings.providerOrder || []).join(",")));
        } catch (err) {
            setSettingsStatus("保存供应商顺序失败：" + (err && err.message ? err.message : String(err)));
        }
        renderAvailableModels();
    }
    var availableTesting = {};

    /// 供应商内全部测试：串行 + 300ms 间隔，可中途停止
    async function testProviderModels(providerId, items, button) {
        if (availableTesting[providerId]) {
            availableTesting[providerId] = false;
            if (button) {
                button.textContent = "全部测试";
            }
            return;
        }
        availableTesting[providerId] = true;
        if (button) {
            button.textContent = "停止";
        }
        for (var i = 0; i < items.length; i += 1) {
            if (!availableTesting[providerId]) {
                break;
            }
            try {
                await invokeFn("test_external_model", { modelId: items[i].id });
            } catch (err) {
                /* 单个失败不影响后续 */
            }
            await new Promise(function (resolve) {
                window.setTimeout(resolve, 300);
            });
        }
        availableTesting[providerId] = false;
        await loadSettings();
        renderAvailableModels();
        renderComposerModel();
    }
    async function persistAvailableModels() {
        if (!settings) {
            return;
        }
        var next = JSON.parse(JSON.stringify(settings));
        try {
            var result = await invokeFn("save_settings", { settings: next });
            settingsView = result && result.view ? result.view : settingsView;
            settings = settingsView && settingsView.settings ? settingsView.settings : next;
        } catch (err) {
            setSettingsStatus("保存可用模型失败：" + (err && err.message ? err.message : String(err)));
        }
    }
    /// 模型目录行：左列显示名可改，右列官方 id 只读
    var providerDisplayNames = {};

    function parseModelIds() {
        return els.providerFormModels.value.split(",").map(function (item) {
            return item.trim();
        }).filter(Boolean);
    }

    function renderProviderModelRows() {
        var box = document.getElementById("provider-model-rows");
        if (!box) {
            return;
        }
        box.textContent = "";
        parseModelIds().forEach(function (id) {
            var row = document.createElement("div");
            row.className = "provider-model-row";

            var display = document.createElement("input");
            display.type = "text";
            display.value = providerDisplayNames[id] || id;
            display.placeholder = "显示名";
            display.addEventListener("input", function () {
                providerDisplayNames[id] = display.value.trim();
            });

            var official = document.createElement("input");
            official.type = "text";
            official.value = id;
            official.readOnly = true;
            official.title = "官方 id，请求时使用，不建议修改";

            var savedIds = ((settings && settings.externalModels) || [])
                .filter(function (item) {
                    return item.providerId === els.providerFormId.value.trim();
                })
                .map(function (item) {
                    return item.modelId;
                });
            if (savedIds.indexOf(id) === -1) {
                var untested = document.createElement("span");
                untested.className = "provider-tag";
                untested.textContent = "未测";
                row.appendChild(untested);
            }

            var del = document.createElement("button");
            del.type = "button";
            del.className = "ghost";
            del.textContent = "删除";
            del.addEventListener("click", function () {
                var rest = parseModelIds().filter(function (item) {
                    return item !== id;
                });
                els.providerFormModels.value = rest.join(", ");
                renderProviderModelRows();
            });

            row.appendChild(display);
            row.appendChild(official);
            row.appendChild(del);
            box.appendChild(row);
        });
    }

    async function fetchProviderModels() {
        if (!invokeFn) {
            return;
        }
        var baseUrl = els.providerFormBase.value.trim();
        if (!baseUrl) {
            toast("请先填写 Base URL");
            return;
        }
        var apiKey = els.providerFormKey.value.trim();
        var providerId = els.providerFormId.value.trim();
        els.providerFetchModels.disabled = true;
        if (els.providerModelsStatus) {
            els.providerModelsStatus.textContent = "获取中…";
        }
        try {
            var models = await invokeFn("fetch_provider_models", {
                baseUrl: baseUrl,
                apiKey: apiKey ? apiKey : null,
                providerId: providerId ? providerId : null
            });
            if (!models || !models.length) {
                if (els.providerModelsStatus) {
                    els.providerModelsStatus.textContent = "未获取到模型：该接口可能不支持 /models，请在「高级」里手动填写模型 id";
                }
                return;
            }
            renderProviderModelRows();
            // 拉取成功：默认全选，用户再取消不要的
            if (!els.providerFormModels.value.trim()) {
                els.providerFormModels.value = models.join(", ");
            }
            renderModelPicker(models);
            if (els.providerModelsStatus) {
                els.providerModelsStatus.textContent =
                    "共 " + models.length + " 个模型，勾选后点「应用所选」";
            }
        } catch (err) {
            var message = err && err.message ? err.message : String(err);
            if (els.providerModelsStatus) {
                els.providerModelsStatus.textContent = "获取失败：" + message;
            }
            toast("获取模型失败：" + message);
        } finally {
            els.providerFetchModels.disabled = false;
        }
    }

    function openProviderForm(provider) {
        if (!els.providerForm) {
            return;
        }
        els.providerForm.hidden = false;
        els.providerFormId.value = provider ? provider.id : "";
        els.providerFormId.disabled = Boolean(provider);
        els.providerFormName.value = provider ? provider.name : "";
        els.providerFormBase.value = provider ? provider.baseUrl : "";
        els.providerFormModels.value = provider ? (provider.models || []).join(", ") : "";
        providerDisplayNames = {};
        (settings && settings.externalModels ? settings.externalModels : []).forEach(function (item) {
            if (provider && item.providerId === provider.id && item.displayName) {
                providerDisplayNames[item.modelId] = item.displayName;
            }
        });
        renderProviderModelRows();
        if (els.providerModelsPicker) {
            els.providerModelsPicker.hidden = true;
            els.providerModelsPicker.textContent = "";
        }
        if (els.providerModelsStatus) {
            els.providerModelsStatus.textContent = "";
        }
        if (els.providerFormPreset) {
            els.providerFormPreset.value = "";
            if (provider) {
                // 按已保存的 Base URL 反查预设，回填供应商下拉
                var savedBase = (provider.baseUrl || "").replace(/\/+$/, "");
                (providerPresets || []).forEach(function (item) {
                    var candidate = (item.baseUrl || "").replace(/\/+$/, "");
                    if (candidate && candidate === savedBase) {
                        els.providerFormPreset.value = item.id;
                    }
                });
            }
        }
        els.providerFormDefault.value = provider ? (provider.defaultModel || "") : "";
        els.providerFormKey.value = "";
        els.providerFormEnabled.checked = provider ? provider.enabled : true;
        els.providerFormName.focus();
    }

    function closeProviderForm() {
        if (!els.providerForm) {
            return;
        }
        els.providerForm.hidden = true;
        els.providerForm.reset();
        els.providerFormId.disabled = false;
    }

    async function submitProviderForm(event) {
        event.preventDefault();
        if (!invokeFn) {
            setSettingsStatus("浏览器预览：无法保存 Provider");
            return;
        }
        var provider = {
            id: els.providerFormId.value.trim(),
            name: els.providerFormName.value.trim(),
            baseUrl: els.providerFormBase.value.trim(),
            models: els.providerFormModels.value.split(",").map(function (item) {
                return item.trim();
            }).filter(Boolean),
            defaultModel: els.providerFormDefault.value.trim(),
            enabled: els.providerFormEnabled.checked,
            apiKey: els.providerFormKey.value.trim() ? els.providerFormKey.value.trim() : null
        };
        if (!provider.id || !provider.name || !provider.baseUrl) {
            setSettingsStatus("Provider ID、名称和 Base URL 不能为空");
            return;
        }
        try {
            await invokeFn("save_provider", { provider: provider });
            // 先重新加载设置，避免用旧快照写 externalModels 时把刚保存的 Provider 覆盖掉
            await loadSettings();
            await persistProviderModels(provider.id);
            try {
                await invokeFn("test_provider", { providerId: provider.id, force: true });
            } catch (err) {
                /* 健康探测失败不影响保存结果 */
            }
            await loadProviders();
            await loadSettings();
            closeProviderForm();
            setSettingsStatus("Provider 已保存");
        } catch (err) {
            setSettingsStatus("保存 Provider 失败：" + (err && err.message ? err.message : String(err)));
        }
    }

    async function testProvider(provider, button) {
        if (!invokeFn) {
            return;
        }
        button.disabled = true;
        button.textContent = "测试中…";
        try {
            var result = await invokeFn("test_provider", { providerId: provider.id, force: true });
            provider.reachable = result && result.reachable;
            toast(result && result.reachable
                ? provider.name + " 可达"
                : provider.name + " 不可达：" + (result && result.error || "未知错误"));
            renderProviderList();
        } catch (err) {
            toast("测试失败：" + (err && err.message ? err.message : String(err)));
        } finally {
            button.disabled = false;
        }
    }

    /// 只清除加密密钥，保留 Provider 配置
    async function clearProviderKey(provider) {
        if (!window.confirm("清除「" + provider.name + "」的 API Key？\n\nProvider 配置会保留，但之后需要重新填写并验证 Key 才能使用。")) {
            return;
        }
        try {
            await invokeFn("clear_provider_key", { providerId: provider.id });
            try {
                await invokeFn("test_provider", { providerId: provider.id, force: true });
            } catch (err) {
                /* 健康探测失败不影响保存结果 */
            }
            await loadProviders();
            await loadSettings();
            setSettingsStatus("已清除密钥");
        } catch (err) {
            setSettingsStatus("清除密钥失败：" + (err && err.message ? err.message : String(err)));
        }
    }

    async function removeProvider(provider) {
        if (!window.confirm("删除 Provider「" + provider.name + "」？相关 API Key 与导出授权也会一并删除。")) {
            return;
        }
        try {
            await invokeFn("delete_provider", { providerId: provider.id });
            try {
                await invokeFn("test_provider", { providerId: provider.id, force: true });
            } catch (err) {
                /* 健康探测失败不影响保存结果 */
            }
            await loadProviders();
            await loadSettings();
            setSettingsStatus("Provider 已删除");
        } catch (err) {
            setSettingsStatus("删除 Provider 失败：" + (err && err.message ? err.message : String(err)));
        }
    }

    async function revokeExport(providerId) {
        try {
            await invokeFn("revoke_file_export", { providerId: providerId });
            try {
                await invokeFn("test_provider", { providerId: provider.id, force: true });
            } catch (err) {
                /* 健康探测失败不影响保存结果 */
            }
            await loadProviders();
            await loadSettings();
            setSettingsStatus("已撤销该 Provider 的文件导出授权");
        } catch (err) {
            setSettingsStatus("撤销失败：" + (err && err.message ? err.message : String(err)));
        }
    }

    function showProviderPicker(note, context) {
        if (!els.externalPicker) {
            return;
        }
        providerPickerContext = context;
        els.externalPickerNote.textContent = note || "选择用哪个外部 Provider 继续回答。";
        els.externalPickerList.innerHTML = "";
        var catalog = ((settings && settings.externalModels) || [])
            .filter(function (item) { return item.enabled !== false; })
            .slice()
            .sort(function (a, b) { return (a.sortIndex || 0) - (b.sortIndex || 0); });
        var usable = catalog.map(function (item) {
            var provider = providerById(item.providerId);
            return {
                id: item.id,
                name: (provider ? provider.name : item.providerId) + " · " + (item.displayName || item.modelId),
                defaultModel: item.modelId,
                provider: provider
            };
        });
        usable.forEach(function (provider) {
            var item = document.createElement("div");
            item.className = "provider-item";
            var info = document.createElement("div");
            info.className = "provider-info";
            var name = document.createElement("strong");
            name.textContent = provider.name;
            var detail = document.createElement("span");
            detail.textContent = provider.availableProviders === undefined ? provider.defaultModel : provider.defaultModel;
            info.appendChild(name);
            info.appendChild(detail);
            var actions = document.createElement("div");
            actions.className = "provider-actions";
            var pick = document.createElement("button");
            pick.type = "button";
            pick.className = "primary";
            pick.textContent = "使用";
            pick.addEventListener("click", function () {
                pickProvider(provider.id);
            });
            actions.appendChild(pick);
            item.appendChild(info);
            item.appendChild(actions);
            els.externalPickerList.appendChild(item);
        });
        if (!usable.length) {
            var empty = document.createElement("div");
            empty.className = "settings-hint";
            empty.textContent = "没有可用的外部 Provider。请先在设置页配置并启用。";
            els.externalPickerList.appendChild(empty);
        }
        els.externalPicker.hidden = false;
    }

    function closeProviderPicker() {
        providerPickerContext = null;
        if (els.externalPicker) {
            els.externalPicker.hidden = true;
        }
    }

    async function pickProvider(providerId) {
        var context = providerPickerContext;
        closeProviderPicker();
        if (!context) {
            return;
        }
        els.sendBtn.disabled = true;
        els.sendBtn.textContent = "发送中…";
        var typing = showTyping();
        try {
            var reply = await executeExternal(context, typing);
            if (reply) {
                typing.remove();
                applyExternalReply(context, reply);
            }
        } catch (err) {
            typing.remove();
            addMessage(context.role, context.sessionId, "error",
                "外部模型调用失败：" + (err && err.message ? err.message : String(err)),
                "小明同学 · " + timeText());
        } finally {
            els.sendBtn.disabled = false;
            els.sendBtn.textContent = "发送";
        }
    }

    function showExportModal(gate, context) {
        exportContext = context;
        if (!els.exportPanel) {
            return;
        }
        els.exportPathList.innerHTML = "";
        (gate.paths || []).forEach(function (path) {
            var code = document.createElement("code");
            code.textContent = path;
            els.exportPathList.appendChild(code);
        });
        els.exportPanel.hidden = false;
    }

    function closeExportModal() {
        exportContext = null;
        if (els.exportPanel) {
            els.exportPanel.hidden = true;
        }
        if (els.exportStatus) {
            els.exportStatus.textContent = "";
        }
    }

    async function confirmExport() {
        var context = exportContext;
        if (!context || !invokeFn) {
            return;
        }
        els.exportConfirm.disabled = true;
        els.exportStatus.textContent = "授权中…";
        try {
            await invokeFn("grant_file_export", {
                roleId: context.role,
                providerId: context.providerId
            });
            els.exportPanel.hidden = true;
            els.exportStatus.textContent = "";
            var typing = showTyping();
            var reply = await executeExternal(context, typing);
            if (reply) {
                typing.remove();
                applyExternalReply(context, reply);
            }
        } catch (err) {
            els.exportStatus.textContent = "发送失败：" + (err && err.message ? err.message : String(err));
        } finally {
            els.exportConfirm.disabled = false;
        }
    }

    /// 聊天失败时询问是否把该模型标记为不可用（只对账户/鉴权/模型类错误提示）
    async function offerMarkModelUnavailable(modelId, message) {
        var text = String(message || "");
        var matched = ["401", "402", "404", "余额", "Key", "模型不存在"].some(function (token) {
            return text.indexOf(token) !== -1;
        });
        if (!matched || !modelId) {
            return;
        }
        var confirmed = window.confirm(
            "模型「" + providerName(modelId) + "」调用失败：\n" + text +
            "\n\n是否把它标记为不可用？标记后可随时在「可用模型」页重新测试。");
        if (!confirmed) {
            return;
        }
        try {
            await invokeFn("record_model_error", { modelId: modelId, message: text });
            await loadSettings();
            renderAvailableModels();
        renderComposerModel();
            toast("已标记为不可用：" + providerName(modelId));
        } catch (err) {
            toast("标记失败：" + (err && err.message ? err.message : String(err)));
        }
    }
    async function executeExternal(context, typing) {
        if (!invokeFn) {
            return null;
        }
        var gate = await cloudGate(context.sessionId);
        if (gate === "cancel") {
            if (typing) {
                typing.remove();
            }
            return null;
        }
        if (gate === "local") {
            if (typing) {
                typing.remove();
            }
            toast("已取消云端调用：请在本地重新发送这条消息");
            return null;
        }
        var outcome;
        try {
            outcome = await invokeFn("send_external_message", {
                role: context.role,
                content: context.message,
                sessionId: context.sessionId,
                providerId: context.providerId,
                includeContext: context.includeContext !== false
            });
        } catch (err) {
            if (typing) {
                typing.remove();
            }
            await offerMarkModelUnavailable(context.providerId, err && err.message ? err.message : String(err));
            throw err;
        }
        if (outcome && outcome.status === "exportRequired" && outcome.gate) {
            if (typing) {
                typing.remove();
            }
            showExportModal(outcome.gate, context);
            return null;
        }
        if (outcome && outcome.status === "ok" && outcome.reply) {
            return outcome.reply;
        }
        throw new Error("外部模型返回异常：" + (outcome && outcome.status));
    }

    function applyExternalReply(context, reply) {
        var meta = "小明同学 · " + providerName(context.providerId) +
            " · " + reply.model + " · " + reply.elapsedMs + "ms" +
            usageText(reply.usage);
        addMessage(context.role, context.sessionId, "assistant", reply.content, meta);
        updateUsageCard(context.providerId, reply.usage);
        if (context.sessionId === currentSessionId()) {
            renderSessions();
        }
    }

    function shouldSuggestExternal(userText, content) {
        if (!content || content.length >= 50) {
            return false;
        }
        var refusal = /抱歉|无法|不能|sorry|can't|cannot|unable|不理解|太复杂|超出/.test(content);
        if (!refusal) {
            return false;
        }
        if (userText.length > 100) {
            return true;
        }
        return /代码|报错|bug|正则|脚本|函数|class |function |import |多文件|跨语言|数学|推导|重构|数据库|sql/.test(userText);
    }

    function suggestHint(sessionId, userText, content) {
        if (!shouldSuggestExternal(userText, content)) {
            return;
        }
        var count = hintCountBySession[sessionId] || 0;
        if (count >= 2) {
            return;
        }
        hintCountBySession[sessionId] = count + 1;
        toast("这个回答可能不完整，可以用「外部模型重答」试试。");
    }

    function copyText(text) {
        if (!text) {
            return;
        }
        if (navigator.clipboard && navigator.clipboard.writeText) {
            navigator.clipboard.writeText(text).then(function () {
                toast("已复制");
            }).catch(function () {
                fallbackCopy(text);
            });
        } else {
            fallbackCopy(text);
        }
    }

    function fallbackCopy(text) {
        var textarea = document.createElement("textarea");
        textarea.value = text;
        textarea.style.position = "fixed";
        textarea.style.opacity = "0";
        document.body.appendChild(textarea);
        textarea.select();
        try {
            document.execCommand("copy");
            toast("已复制");
        } catch (err) {
            toast("复制失败");
        }
        document.body.removeChild(textarea);
    }

    function lastUserMessage() {
        var list = currentMessages();
        for (var i = list.length - 1; i >= 0; i -= 1) {
            if (list[i].kind === "user") {
                return list[i].content;
            }
        }
        return null;
    }

    async function regenerate() {
        var text = lastUserMessage();
        if (!text || sending) {
            return;
        }
        var sessionId = currentSessionId();
        if (!sessionId) {
            return;
        }
        var roleAtSend = currentRole;
        taskRole = roleAtSend;
        sending = true;
        els.sendBtn.disabled = true;
        els.sendBtn.textContent = "发送中…";
        updateStatus();
        requestRoute(text);
        var typing = showTyping();
        var started = window.performance.now();
        await runSendCore(roleAtSend, sessionId, text, typing, started, true);
        finishSend();
    }

    function reanswerExternal() {
        var text = lastUserMessage();
        if (!text || sending) {
            return;
        }
        var sessionId = currentSessionId();
        if (!sessionId) {
            return;
        }
        var roleAtSend = currentRole;
        var usable = availableProviders();
        if (!usable.length) {
            toast("没有可用的外部 Provider，请先在设置页配置。");
            return;
        }
        taskRole = roleAtSend;
        sending = true;
        els.sendBtn.disabled = true;
        els.sendBtn.textContent = "发送中…";
        updateStatus();
        var typing = showTyping();
        var context = {
            role: roleAtSend,
            message: text,
            sessionId: sessionId,
            providerId: usable.length === 1 ? usable[0].id : null,
            includeContext: true
        };
        if (context.providerId) {
            executeExternal(context, typing).then(function (reply) {
                if (reply) {
                    typing.remove();
                    applyExternalReply(context, reply);
                    finishSend();
                } else {
                    finishSend();
                }
            }).catch(function (err) {
                typing.remove();
                addMessage(roleAtSend, sessionId, "error",
                    "外部模型调用失败：" + (err && err.message ? err.message : String(err)),
                    "小明同学 · " + timeText());
                finishSend();
            });
        } else {
            typing.remove();
            showProviderPicker("选择一个外部 Provider 重答：", context);
            finishSend();
        }
    }

    function finishSend() {
        sending = false;
        els.sendBtn.disabled = false;
        els.sendBtn.textContent = "发送";
        els.input.focus();
        updateStatus();
    }

    function bindEvents() {
        els.sendBtn.addEventListener("click", sendMessage);
        startUsageTimer();
        bindComposerModelPicker();
        els.clearBtn.addEventListener("click", clearCurrent);
        if (els.localOnlyBtn) {
            els.localOnlyBtn.addEventListener("click", toggleLocalOnly);
        }
        if (els.onboardingNext) {
            els.onboardingNext.addEventListener("click", nextOnboarding);
        }
        if (els.onboardingSkip) {
            els.onboardingSkip.addEventListener("click", function () {
                finishOnboarding(true);
            });
        }
        if (els.cloudAuditRefresh) {
            els.cloudAuditRefresh.addEventListener("click", loadCloudAudit);
        }
        if (els.openDataDir) {
            els.openDataDir.addEventListener("click", function () {
                openLocalPath("data", els.settingsDataStatus);
            });
        }
        if (els.pickDataDir) {
            els.pickDataDir.addEventListener("click", pickDataDir);
        }
        if (els.settingsDataSave) {
            els.settingsDataSave.addEventListener("click", saveDataRoot);
        }
        if (els.settingsDataCancel) {
            els.settingsDataCancel.addEventListener("click", cancelPendingDataRoot);
        }
        if (els.restartNow) {
            els.restartNow.addEventListener("click", doCloseApp);
        }
        var gateBindings = [
            ["cloud-gate-local", "local"],
            ["cloud-gate-once", "once"],
            ["cloud-gate-never", "never"],
            ["cloud-gate-cancel", "cancel"]
        ];
        gateBindings.forEach(function (pair) {
            var button = document.getElementById(pair[0]);
            if (button) {
                button.addEventListener("click", function () {
                    if (cloudGateResolve) {
                        cloudGateResolve(pair[1]);
                    }
                });
            }
        });
        if (els.restartAbort) {
            els.restartAbort.addEventListener("click", abortDataSwitch);
        }
        var dataClose = document.getElementById("settings-data-close");
        if (dataClose) {
            dataClose.addEventListener("click", doCloseApp);
        }
        var dataAbort = document.getElementById("settings-data-abort");
        if (dataAbort) {
            dataAbort.addEventListener("click", abortDataSwitch);
        }
        els.newSessionBtn.addEventListener("click", function () {
            createSession(currentRole);
        });
        els.roleWheelBtn.addEventListener("click", function (event) {
            event.stopPropagation();
            toggleRoleWheel();
        });
        els.roleWheelList.addEventListener("scroll", onRoleWheelScroll);
        els.roleWheelList.addEventListener("keydown", onRoleWheelKeydown);
        document.addEventListener("click", function (event) {
            if (!roleWheelOpen) {
                return;
            }
            if (els.roleWheel.contains(event.target) || els.roleWheelBtn.contains(event.target)) {
                return;
            }
            closeRoleWheel();
        });

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

        els.settingsBtn.addEventListener("click", function () {
            openSettings("general");
        });
        els.settingsClose.addEventListener("click", closeSettings);
        els.settingsSave.addEventListener("click", saveSettings);
        els.settingsThemeSelect.addEventListener("change", function () {
            applyTheme(els.settingsThemeSelect.value);
        });
        els.roleAddBtn.addEventListener("click", function () {
            openRoleForm(null);
        });
        els.roleFormCancel.addEventListener("click", closeRoleForm);
        els.roleForm.addEventListener("submit", submitRoleForm);
        if (els.roleFormGenerate) {
            els.roleFormGenerate.addEventListener("click", generateRolePrompt);
        }
        if (els.roleFormGenerateStatus) {
            els.roleFormGenerateStatus.addEventListener("click", async function () {
                if (!lastGenerateError) {
                    return;
                }
                var copied = await copyText(lastGenerateError);
                toast(copied ? "失败原因已复制" : "复制失败，请查看 data/logs 下的日志");
            });
        }
        els.settingsPanel.addEventListener("click", function (event) {
            if (event.target === els.settingsPanel) {
                closeSettings();
            }
        });

        els.providerAddBtn.addEventListener("click", function () {
            openProviderForm(null);
        });
        els.providerFormCancel.addEventListener("click", closeProviderForm);
        els.providerForm.addEventListener("submit", submitProviderForm);
        if (els.providerFormPreset) {
            els.providerFormPreset.addEventListener("change", applyProviderPreset);
        }
        var addModelBtn = document.getElementById("provider-add-model");
        if (addModelBtn) {
            addModelBtn.addEventListener("click", openAddModelModal);
        }
        ["provider-add-close", "provider-add-cancel"].forEach(function (id) {
            var btn = document.getElementById(id);
            if (btn) {
                btn.addEventListener("click", closeAddModelModal);
            }
        });
        var addConfirm = document.getElementById("provider-add-confirm");
        if (addConfirm) {
            addConfirm.addEventListener("click", function () {
                var idInput = document.getElementById("provider-add-id");
                var nameInput = document.getElementById("provider-add-name");
                var id = idInput ? idInput.value.trim() : "";
                if (!id) {
                    toast("请填写官方模型 id");
                    return;
                }
                var ids = parseModelIds();
                if (ids.indexOf(id) === -1) {
                    ids.push(id);
                }
                els.providerFormModels.value = ids.join(", ");
                if (nameInput && nameInput.value.trim()) {
                    providerDisplayNames[id] = nameInput.value.trim();
                }
                renderProviderModelRows();
                closeAddModelModal();
            });
        }        var testConn = document.getElementById("provider-test-conn");
        if (testConn) {
            testConn.addEventListener("click", testProviderConnection);
        }
        var pickSearch = document.getElementById("provider-picker-search");
        if (pickSearch) {
            pickSearch.addEventListener("input", function () {
                renderPickerList(pickSearch.value);
            });
        }
        var pickToggle = document.getElementById("provider-picker-toggle");
        if (pickToggle) {
            pickToggle.addEventListener("click", function () {
                var list = document.getElementById("provider-picker-list");
                var boxes = list ? list.querySelectorAll('input[type="checkbox"]') : [];
                var allChecked = true;
                Array.prototype.forEach.call(boxes, function (box) {
                    if (!box.checked) {
                        allChecked = false;
                    }
                });
                Array.prototype.forEach.call(boxes, function (box) {
                    box.checked = !allChecked;
                });
                pickToggle.textContent = allChecked ? "全选" : "取消全选";
                updatePickerCount();
            });
        }
        ["provider-picker-close", "provider-picker-cancel"].forEach(function (id) {
            var btn = document.getElementById(id);
            if (btn) {
                btn.addEventListener("click", closeModelPicker);
            }
        });
        var pickApply = document.getElementById("provider-picker-apply");
        if (pickApply) {
            pickApply.addEventListener("click", applyPickedModels);
        }
        if (els.providerFetchModels) {
            els.providerFetchModels.addEventListener("click", openModelPicker);
        }
        if (els.openModelsDir) {
            els.openModelsDir.addEventListener("click", function () {
                openLocalPath("models");
            });
        }
        if (els.openLogsDir) {
            els.openLogsDir.addEventListener("click", function () {
                openLocalPath("logs", els.settingsLogsStatus);
            });
        }
        if (els.scanModelsBtn) {
            els.scanModelsBtn.addEventListener("click", scanModels);
        }
        if (els.modelImportCancel) {
            els.modelImportCancel.addEventListener("click", closeModelImport);
        }
        if (els.modelImportSubmit) {
            els.modelImportSubmit.addEventListener("click", submitModelImport);
        }
        if (els.modelImportCtx) {
            els.modelImportCtx.addEventListener("input", function () {
                if (!pendingImport || !els.modelImportPreview) {
                    return;
                }
                var ctx = parseInt(els.modelImportCtx.value, 10);
                var text = "FROM ./" + pendingImport.relative + "\n";
                if (!isNaN(ctx) && ctx > 0) {
                    text += "PARAMETER num_ctx " + ctx + "\n";
                }
                els.modelImportPreview.textContent = text;
            });
        }
        els.externalPickerClose.addEventListener("click", closeProviderPicker);
        els.exportClose.addEventListener("click", closeExportModal);
        els.exportConfirm.addEventListener("click", confirmExport);

        var settingsTabs = document.querySelectorAll(".settings-tab");
        for (var tabIndex = 0; tabIndex < settingsTabs.length; tabIndex += 1) {
            settingsTabs[tabIndex].addEventListener("click", function (event) {
                showSettingsTab(event.currentTarget.dataset.tab);
                if (event.currentTarget.dataset.tab === "external") {
                    loadProviders();
                }
            });
        }

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
        els.sessionRole = $("session-role");
        els.sessionList = $("session-list");
        els.newSessionBtn = $("new-session-btn");
        els.roleWheelBtn = $("role-wheel-btn");
        els.roleWheelLabel = $("role-wheel-label");
        els.roleWheel = $("role-wheel");
        els.roleWheelList = $("role-wheel-list");
        els.settingsBtn = $("settings-btn");
        els.settingsPanel = $("settings-panel");
        els.settingsClose = $("settings-close");
        els.settingsThemeSelect = $("settings-theme-select");
        els.settingsOllamaHost = $("settings-ollama-host");
        els.settingsRouter = $("settings-router");
        els.settingsCoordinator = $("settings-coordinator");
        els.openDataDir = $("open-data-dir");
        els.pickDataDir = $("pick-data-dir");
        els.settingsDataName = $("settings-data-name");
        els.settingsDataPendingRow = $("settings-data-pending-row");
        els.settingsDataPending = $("settings-data-pending");
        els.settingsDataSave = $("settings-data-save");
        els.settingsDataCancel = $("settings-data-cancel");
        els.settingsStatus = $("settings-status");
        els.settingsSave = $("settings-save");
        els.settingsRoleList = $("settings-role-list");
        els.roleAddBtn = $("role-add-btn");
        els.roleForm = $("role-form");
        els.roleFormIdHint = $("role-form-id-hint");
        els.roleFormGenerate = $("role-form-generate");
        els.roleFormGenerateStatus = $("role-form-generate-status");
        els.roleFormName = $("role-form-name");
        els.roleFormDesc = $("role-form-desc");
        els.roleFormPrompt = $("role-form-prompt");
        els.roleFormCancel = $("role-form-cancel");
        els.settingsDefaultChat = $("settings-default-chat");
        els.settingsCloudMode = $("settings-cloud-mode");
        els.providerList = $("provider-list");
        els.providerAddBtn = $("provider-add-btn");
        els.providerForm = $("provider-form");
        els.providerFormId = $("provider-form-id");
        els.providerFormName = $("provider-form-name");
        els.providerFormBase = $("provider-form-base");
        els.providerFormModels = $("provider-form-models");
        els.providerFormPreset = $("provider-form-preset");
        els.providerFetchModels = $("provider-fetch-models");
        els.providerModelsStatus = $("provider-models-status");
        els.providerModelsPicker = $("provider-models-picker");
        els.openModelsDir = $("open-models-dir");
        els.scanModelsBtn = $("scan-models-btn");
        els.modelsScanStatus = $("models-scan-status");
        els.modelsScanList = $("models-scan-list");
        els.modelImportForm = $("model-import-form");
        els.modelImportName = $("model-import-name");
        els.modelImportGrade = $("model-import-grade");
        els.modelImportCtx = $("model-import-ctx");
        els.modelImportDisplay = $("model-import-display");
        els.modelImportPreview = $("model-import-preview");
        els.modelImportCancel = $("model-import-cancel");
        els.modelImportSubmit = $("model-import-submit");
        els.openLogsDir = $("open-logs-dir");
        els.providerFormDefault = $("provider-form-default");
        els.providerFormKey = $("provider-form-key");
        els.providerFormEnabled = $("provider-form-enabled");
        els.providerFormCancel = $("provider-form-cancel");
        els.exportGrantList = $("export-grant-list");
        els.externalPicker = $("external-picker");
        els.externalPickerNote = $("external-picker-note");
        els.externalPickerList = $("external-picker-list");
        els.externalPickerClose = $("external-picker-close");
        els.exportPanel = $("export-panel");
        els.exportPathList = $("export-path-list");
        els.exportClose = $("export-close");
        els.exportConfirm = $("export-confirm");
        els.exportStatus = $("export-status");
        els.toast = $("toast");
        els.usageName = $("usage-name");
        els.usageValue = $("usage-value");
        els.roleTitle = $("role-title");
        els.roleDesc = $("role-desc");
        els.history = $("chat-history");
        els.input = $("input-text");
        els.sendBtn = $("send-btn");
        els.clearBtn = $("clear-btn");
        els.localOnlyBtn = $("local-only-btn");
        els.settingsCloudBudget = $("settings-cloud-budget");
        els.settingsCloudProfile = $("settings-cloud-profile");
        els.cloudAuditRefresh = $("cloud-audit-refresh");
        els.cloudAuditList = $("cloud-audit-list");
        els.settingsDataStatus = $("settings-data-status");
        els.settingsLogsStatus = $("settings-logs-status");
        els.restartNotice = $("restart-notice");
        els.restartNoticeText = $("restart-notice-text");
        els.restartNow = $("restart-now");
        els.restartAbort = $("restart-abort");
        els.onboarding = $("onboarding");
        els.onboardingTitle = $("onboarding-title");
        els.onboardingBody = $("onboarding-body");
        els.onboardingList = $("onboarding-list");
        els.onboardingStepLabel = $("onboarding-step");
        els.onboardingSkip = $("onboarding-skip");
        els.onboardingNext = $("onboarding-next");
        els.statusBar = $("status-bar");
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
            if (!sessionsByRole[role.id]) {
                sessionsByRole[role.id] = [];
            }
        });

        var savedTheme = "system";
        try {
            savedTheme = localStorage.getItem("ai-assistant-theme") || "system";
        } catch (err) {
            savedTheme = "system";
        }

        applyTheme(savedTheme);
        renderRole();
        renderSessions();
        renderHistory();
        bindEvents();
        listenPermissions();
        listenTaskProgress();
        updateStatus();
        loadRoles().then(function () {
            loadSessions(currentRole);
        });
        probeBackend().then(function () {
            checkEnvironment();
        });
        loadSettings();
        loadProviders();
        loadProviderPresets();
        loadLocalModels();
    }

    if (document.readyState === "loading") {
        document.addEventListener("DOMContentLoaded", init);
    } else {
        init();
    }
})();
