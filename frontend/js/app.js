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
    var localModels = [];
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
            builtin: Boolean(item.builtin)
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

    function roleName(id) {
        var role = roleById(id);
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

    async function toggleLocalOnly() {
        if (!invokeFn || !settings) {
            toast("设置不可用，无法切换模式");
            return;
        }
        var next = !autoEscalateOn(); // true = 关闭「仅本地」，切到自动模式
        if (next) {
            var confirmed = window.confirm(
                "关闭「仅本地」后：\n\n" +
                "· 认知复杂的任务（长文档高质量总结、跨文件重构、架构分析、开放式多步规划）会直接使用云端，不再逐次询问；\n" +
                "· 敏感内容（secrets.json / .env / .ssh / 密钥文件）与疑似密钥仍受红线保护，绝不上云；\n" +
                "· 每次出网都会写入审计日志；会话云端预算用尽后自动降级为仅本地。\n\n" +
                "确定切换到自动模式吗？");
            if (!confirmed) {
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
        if (els.settingsCloudBudget) {
            var budget = settings.cloud.budgetTokens;
            els.settingsCloudBudget.value = String(budget == null ? 50000 : budget);
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
    async function migrateDataRoot() {
        if (!invokeFn) {
            toast("浏览器预览：无法迁移");
            return;
        }
        var target = els.settingsDataTarget ? els.settingsDataTarget.value.trim() : "";
        if (!target) {
            toast("请填写目标目录");
            return;
        }
        var confirmed = window.confirm(
            "将把当前数据复制到：\n" + target + "\n\n" +
            "· 目标目录已存在的同名文件会被覆盖；\n" +
            "· 复制后会逐字节校验，校验不通过则不写指针；\n" +
            "· 迁移完成后需要重启应用才生效。\n\n继续吗？");
        if (!confirmed) {
            return;
        }
        els.settingsDataMigrate.disabled = true;
        if (els.settingsDataStatus) {
            els.settingsDataStatus.textContent = "迁移中…";
        }
        try {
            var report = await invokeFn("set_data_root", { path: target });
            var sizeKb = Math.round((report.bytes || 0) / 1024);
            var text = "已复制 " + report.files + " 个文件（" + sizeKb + " KB）";
            if (report.skipped) {
                text += "，跳过 " + report.skipped + " 个被占用文件";
            }
            text += report.verified ? "，校验通过；请重启应用生效" : "，校验未通过";
            if (els.settingsDataStatus) {
                els.settingsDataStatus.textContent = text;
            }
            toast("迁移完成，重启应用后生效");
        } catch (err) {
            var message = err && err.message ? err.message : String(err);
            if (els.settingsDataStatus) {
                els.settingsDataStatus.textContent = "迁移失败：" + message;
            }
            toast("迁移失败：" + message);
        } finally {
            els.settingsDataMigrate.disabled = false;
        }
    }

    // ---- 5B：首次启动引导（3 步） ----
    function onboardingSteps() {
        return [
            {
                title: "欢迎使用小明同学",
                body: "本地优先的个人 AI 助手：默认只跑本地模型（3B 路由 + 7B 大脑），敏感内容不离开这台机器。",
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

            var desc = document.createElement("span");
            desc.className = "role-wheel-desc";
            desc.textContent = role.desc;

            item.appendChild(name);
            item.appendChild(desc);
            item.addEventListener("click", function () {
                switchRole(role.id);
                closeRoleWheel();
            });
            els.roleWheelList.appendChild(item);
        });
        if (roleWheelOpen) {
            centerRoleWheel(false);
        }
    }

    function centerRoleWheel(smooth) {
        if (!els.roleWheelList) {
            return;
        }
        roleWheelSuppress = true;
        els.roleWheelList.scrollTo({
            top: roleIndex() * 48,
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
        if (!roleWheelOpen || roleWheelSuppress) {
            return;
        }
        window.clearTimeout(roleWheelTimer);
        roleWheelTimer = window.setTimeout(function () {
            var index = Math.round(els.roleWheelList.scrollTop / 48);
            index = Math.max(0, Math.min(roles.length - 1, index));
            if (roles[index] && roles[index].id !== currentRole) {
                switchRole(roles[index].id);
            }
        }, 140);
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
        var tabs = document.querySelectorAll(".settings-tab");
        var panels = document.querySelectorAll(".settings-tab-panel");
        for (var i = 0; i < tabs.length; i += 1) {
            tabs[i].classList.toggle("active", tabs[i].dataset.tab === name);
        }
        for (var j = 0; j < panels.length; j += 1) {
            panels[j].classList.toggle("active", panels[j].dataset.panel === name);
        }
    }

    function fillModelSelect(select, selected) {
        if (!select) {
            return;
        }
        select.innerHTML = "";
        var values = localModels.slice();
        if (selected && values.indexOf(selected) === -1) {
            values.unshift(selected);
        }
        if (!values.length && selected) {
            values = [selected];
        }
        values.forEach(function (model) {
            var option = document.createElement("option");
            option.value = model;
            option.textContent = model;
            select.appendChild(option);
        });
        if (selected) {
            select.value = selected;
        }
    }

    function renderSettingsRoles() {
        if (!els.settingsRoleList) {
            return;
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
        els.settingsModelsDir.textContent = models.modelsDir || "-";
        els.settingsDataRoot.textContent = settingsView && settingsView.dataRoot ? settingsView.dataRoot : "-";
        els.settingsLogsDir.textContent = settingsView && settingsView.logsDir ? settingsView.logsDir : "-";
        fillModelSelect(els.settingsRouter, models.router || "");
        fillModelSelect(els.settingsCoordinator, models.coordinator || "");
        renderSettingsRoles();
        applyTheme(s.theme || "system");
    }

    async function loadLocalModels() {
        if (!invokeFn) {
            return;
        }
        try {
            var list = await invokeFn("list_local_models");
            localModels = Array.isArray(list) ? list : [];
        } catch (err) {
            localModels = [];
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
        editingRoleId = role ? role.id : null;
        els.roleForm.hidden = false;
        els.roleFormId.value = role ? role.id : "";
        els.roleFormId.disabled = Boolean(role);
        els.roleFormName.value = role ? role.name : "";
        els.roleFormDesc.value = role ? role.desc : "";
        els.roleFormPrompt.value = role ? (role.systemPrompt || "") : "";
        els.roleFormName.focus();
    }

    function closeRoleForm() {
        if (!els.roleForm) {
            return;
        }
        editingRoleId = null;
        els.roleForm.hidden = true;
        els.roleForm.reset();
        els.roleFormId.disabled = false;
    }

    async function submitRoleForm(event) {
        event.preventDefault();
        if (!invokeFn) {
            setSettingsStatus("浏览器预览：无法保存角色");
            return;
        }
        var role = {
            id: editingRoleId || els.roleFormId.value.trim(),
            name: els.roleFormName.value.trim(),
            desc: els.roleFormDesc.value.trim(),
            systemPrompt: els.roleFormPrompt.value.trim(),
            modelPreference: "heavy",
            builtin: false,
            enabled: true
        };
        if (!role.id || !role.name || !role.systemPrompt) {
            setSettingsStatus("角色 ID、名称和系统提示词不能为空");
            return;
        }
        try {
            await invokeFn("save_role", { role: role });
            await loadRoles();
            renderSettingsRoles();
            closeRoleForm();
            setSettingsStatus("角色已保存");
        } catch (err) {
            setSettingsStatus("保存角色失败：" + (err && err.message ? err.message : String(err)));
        }
    }

    async function removeRole(role) {
        if (!window.confirm("删除角色「" + role.name + "」？对话数据会归档到 memory/_deleted。")) {
            return;
        }
        if (!invokeFn) {
            return;
        }
        try {
            await invokeFn("delete_role", { roleId: role.id, deleteData: false });
            await loadRoles();
            renderSettingsRoles();
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
        var budget = els.settingsCloudBudget ? parseInt(els.settingsCloudBudget.value, 10) : NaN;
        next.cloud.budgetTokens = isNaN(budget) || budget < 0 ? 0 : budget;
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
        var defaultChat = s.defaultChat || "local";
        if (mode === "cloudOnly") {
            if (defaultChat === "local") {
                return "error";
            }
            var provider = providerById(defaultChat);
            return provider && provider.enabled && provider.hasApiKey ? defaultChat : "error";
        }
        if (defaultChat === "local") {
            return "local";
        }
        var external = providerById(defaultChat);
        return external && external.enabled && external.hasApiKey ? defaultChat : "local";
    }

    function refreshDefaultChatSelect() {
        if (!els.settingsDefaultChat) {
            return;
        }
        var current = els.settingsDefaultChat.value || (settings && settings.defaultChat) || "local";
        els.settingsDefaultChat.innerHTML = "";
        var local = document.createElement("option");
        local.value = "local";
        local.textContent = "本地模型";
        els.settingsDefaultChat.appendChild(local);
        providers.forEach(function (provider) {
            if (!provider.enabled) {
                return;
            }
            var option = document.createElement("option");
            option.value = provider.id;
            option.textContent = provider.name;
            els.settingsDefaultChat.appendChild(option);
        });
        if (current !== "local" && !providerById(current)) {
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
            name.textContent = provider.name + (provider.enabled ? "" : " · 已停用");
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

    async function removeProvider(provider) {
        if (!window.confirm("删除 Provider「" + provider.name + "」？相关 API Key 与导出授权也会一并删除。")) {
            return;
        }
        try {
            await invokeFn("delete_provider", { providerId: provider.id });
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
        var usable = availableProviders();
        usable.forEach(function (provider) {
            var item = document.createElement("div");
            item.className = "provider-item";
            var info = document.createElement("div");
            info.className = "provider-info";
            var name = document.createElement("strong");
            name.textContent = provider.name;
            var detail = document.createElement("span");
            detail.textContent = provider.defaultModel;
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

    async function executeExternal(context, typing) {
        if (!invokeFn) {
            return null;
        }
        var outcome = await invokeFn("send_external_message", {
            role: context.role,
            content: context.message,
            sessionId: context.sessionId,
            providerId: context.providerId,
            includeContext: context.includeContext !== false
        });
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
        if (els.settingsDataMigrate) {
            els.settingsDataMigrate.addEventListener("click", migrateDataRoot);
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

        els.themeBtn.addEventListener("click", toggleTheme);
        els.settingsBtn.addEventListener("click", function () {
            openSettings("general");
        });
        els.settingsClose.addEventListener("click", closeSettings);
        els.usageManage.addEventListener("click", function () {
            openSettings("external");
        });
        els.settingsSave.addEventListener("click", saveSettings);
        els.settingsThemeSelect.addEventListener("change", function () {
            applyTheme(els.settingsThemeSelect.value);
        });
        els.roleAddBtn.addEventListener("click", function () {
            openRoleForm(null);
        });
        els.roleFormCancel.addEventListener("click", closeRoleForm);
        els.roleForm.addEventListener("submit", submitRoleForm);
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
        els.usageManage = $("usage-manage");
        els.settingsPanel = $("settings-panel");
        els.settingsClose = $("settings-close");
        els.settingsThemeSelect = $("settings-theme-select");
        els.settingsOllamaHost = $("settings-ollama-host");
        els.settingsRouter = $("settings-router");
        els.settingsCoordinator = $("settings-coordinator");
        els.settingsModelsDir = $("settings-models-dir");
        els.settingsDataRoot = $("settings-data-root");
        els.settingsLogsDir = $("settings-logs-dir");
        els.settingsStatus = $("settings-status");
        els.settingsSave = $("settings-save");
        els.settingsRoleList = $("settings-role-list");
        els.roleAddBtn = $("role-add-btn");
        els.roleForm = $("role-form");
        els.roleFormId = $("role-form-id");
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
        els.settingsDataTarget = $("settings-data-target");
        els.settingsDataMigrate = $("settings-data-migrate");
        els.settingsDataStatus = $("settings-data-status");
        els.onboarding = $("onboarding");
        els.onboardingTitle = $("onboarding-title");
        els.onboardingBody = $("onboarding-body");
        els.onboardingList = $("onboarding-list");
        els.onboardingStepLabel = $("onboarding-step");
        els.onboardingSkip = $("onboarding-skip");
        els.onboardingNext = $("onboarding-next");
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
    }

    if (document.readyState === "loading") {
        document.addEventListener("DOMContentLoaded", init);
    } else {
        init();
    }
})();
