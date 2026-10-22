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
        if (!request || !request.id) {
            return;
        }
        if (activePermission) {
            permissionQueue.push(request);
            return;
        }

        activePermission = request;
        els.permLevel.textContent = request.level || "L2";
        els.permPath.textContent = request.path || "";
        els.permPreview.textContent = request.preview || "";
        els.permPanel.hidden = false;
        pendingConfirm += 1;
        updateStatus("等待写入确认");
    }

    function resolvePermission(approved) {
        var request = activePermission;
        if (!request) {
            return;
        }

        activePermission = null;
        els.permPanel.hidden = true;
        pendingConfirm = Math.max(0, pendingConfirm - 1);

        if (invokeFn) {
            invokeFn("resolve_permission", {
                id: request.id,
                approved: approved
            }).catch(function (err) {
                var message = err && err.message ? err.message : String(err);
                updateStatus("确认失败：" + message);
            });
        }

        updateStatus(approved ? "已允许写入" : "已拒绝写入");
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
        eventApi.listen("permission-request", function (event) {
            showPermission(event && event.payload);
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
            resolvePermission(true);
        });
        els.permDeny.addEventListener("click", function () {
            resolvePermission(false);
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
            realOption.disabled = !realReady;

            if (realReady) {
                els.backendInfo.textContent = status.routerModel + " / " + status.coordinatorModel;
                els.backendInfo.title = "Ollama: " + status.ollamaBaseUrl + "\n数据目录: " + status.dataDir;
                updateStatus("Ollama " + status.ollamaBaseUrl);
            } else {
                els.backendInfo.textContent = "Ollama 未连接";
                els.backendInfo.title = "Ollama: " + status.ollamaBaseUrl;
                updateStatus("请先启动 Ollama");
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
        els.permDeny = $("perm-deny");

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
        updateStatus();
        loadHistory(currentRole);
        probeBackend();
    }

    if (document.readyState === "loading") {
        document.addEventListener("DOMContentLoaded", init);
    } else {
        init();
    }
})();
