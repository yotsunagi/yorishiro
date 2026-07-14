// Yorishiro hosted admin dashboard -- a deliberately framework-free SPA. Scope is limited to
// login, usage/billing display, and member management (see task #54); it is not a general
// entity/schema/relation browser -- that's what the REST API + Swagger UI (`/docs` on
// yorishiro-server) are for.

const SESSION_KEY = "yorishiro_session";

function apiBase() {
  return (window.YORISHIRO_CONFIG && window.YORISHIRO_CONFIG.apiBase) || "";
}

function getSession() {
  const raw = sessionStorage.getItem(SESSION_KEY);
  return raw ? JSON.parse(raw) : null;
}

function setSession(session) {
  sessionStorage.setItem(SESSION_KEY, JSON.stringify(session));
}

function clearSession() {
  sessionStorage.removeItem(SESSION_KEY);
}

function el(html) {
  const template = document.createElement("template");
  template.innerHTML = html.trim();
  return template.content.firstElementChild;
}

function mount(node) {
  const app = document.getElementById("app");
  app.replaceChildren(node);
}

async function parseErrorMessage(response) {
  try {
    const body = await response.json();
    return body?.error?.message || `request failed (${response.status})`;
  } catch {
    return `request failed (${response.status})`;
  }
}

async function login({ email, password, workspaceId }) {
  const response = await fetch(`${apiBase()}/auth/login`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ email, password, workspace_id: workspaceId }),
  });
  if (!response.ok) {
    throw new Error(await parseErrorMessage(response));
  }
  return response.json();
}

async function fetchTenantOverview(apiKey) {
  const response = await fetch("/hosted/tenant/overview", {
    headers: { authorization: `Bearer ${apiKey}` },
  });
  if (!response.ok) {
    throw new Error(await parseErrorMessage(response));
  }
  return response.json();
}

async function addMember(apiKey, { email, role }) {
  const response = await fetch(`${apiBase()}/api/members`, {
    method: "POST",
    headers: {
      "content-type": "application/json",
      authorization: `Bearer ${apiKey}`,
    },
    body: JSON.stringify({ email, role }),
  });
  if (!response.ok) {
    throw new Error(await parseErrorMessage(response));
  }
  return response.json();
}

function renderLogin(errorMessage) {
  const view = el(`
    <div>
      <h1>Yorishiro Hosted Dashboard</h1>
      <p class="hint">Sign in with the account created via an invite (see /auth/signup).</p>
      <form id="login-form">
        <label>Email
          <input type="email" name="email" required autocomplete="username">
        </label>
        <label>Password
          <input type="password" name="password" required autocomplete="current-password">
        </label>
        <label>Workspace ID
          <input type="text" name="workspaceId" required placeholder="00000000-0000-0000-0000-000000000000">
        </label>
        <p class="hint">Ask a tenant owner for your workspace's id, or find it in your signup response.</p>
        ${errorMessage ? `<p class="error">${errorMessage}</p>` : ""}
        <button type="submit">Sign in</button>
      </form>
    </div>
  `);

  view.querySelector("#login-form").addEventListener("submit", async (event) => {
    event.preventDefault();
    const form = new FormData(event.target);
    try {
      const result = await login({
        email: form.get("email"),
        password: form.get("password"),
        workspaceId: form.get("workspaceId"),
      });
      setSession({ apiKey: result.api_key, email: result.email ?? form.get("email") });
      location.hash = "#/dashboard";
    } catch (err) {
      mount(renderLogin(err.message));
    }
  });

  return view;
}

function renderMembersTable(members) {
  const rows = members
    .map(
      (member) => `
        <tr>
          <td>${member.email}</td>
          <td>${member.display_name ?? ""}</td>
          <td>${member.role}</td>
        </tr>`,
    )
    .join("");
  return `
    <table>
      <thead><tr><th>Email</th><th>Name</th><th>Role</th></tr></thead>
      <tbody>${rows}</tbody>
    </table>
  `;
}

function renderDashboardShell(overview, addMemberError) {
  const view = el(`
    <div>
      <div class="top-bar">
        <h1>Tenant Dashboard</h1>
        <button class="secondary" id="logout-button">Sign out</button>
      </div>
      <p class="hint">Tenant ${overview.tenant_id} &middot; plan: ${overview.plan ?? "self-hosted / unmetered"}</p>

      <div class="stat-grid">
        <div class="stat"><div class="value">${overview.usage.workspace_count}</div><div class="label">workspaces${overview.max_workspaces != null ? ` / ${overview.max_workspaces}` : ""}</div></div>
        <div class="stat"><div class="value">${overview.usage.member_count}</div><div class="label">members</div></div>
        <div class="stat"><div class="value">${overview.usage.entity_count}</div><div class="label">entities</div></div>
      </div>

      <h2>Members</h2>
      ${renderMembersTable(overview.members)}

      <h2>Add a member</h2>
      <p class="hint">The person must already have an account (created via /auth/signup from an invite).</p>
      <form id="add-member-form">
        <label>Email
          <input type="email" name="email" required>
        </label>
        <label>Role
          <select name="role">
            <option value="viewer">Viewer</option>
            <option value="member" selected>Member</option>
            <option value="admin">Admin</option>
            <option value="owner">Owner</option>
          </select>
        </label>
        ${addMemberError ? `<p class="error">${addMemberError}</p>` : ""}
        <button type="submit">Add member</button>
      </form>
    </div>
  `);

  view.querySelector("#logout-button").addEventListener("click", () => {
    clearSession();
    location.hash = "#/login";
  });

  view.querySelector("#add-member-form").addEventListener("submit", async (event) => {
    event.preventDefault();
    const session = getSession();
    const form = new FormData(event.target);
    try {
      await addMember(session.apiKey, {
        email: form.get("email"),
        role: form.get("role"),
      });
      await renderDashboard();
    } catch (err) {
      mount(renderDashboardShell(overview, err.message));
    }
  });

  return view;
}

async function renderDashboard() {
  const session = getSession();
  if (!session) {
    location.hash = "#/login";
    return;
  }

  mount(el(`<p>Loading…</p>`));
  try {
    const overview = await fetchTenantOverview(session.apiKey);
    mount(renderDashboardShell(overview));
  } catch (err) {
    clearSession();
    mount(renderLogin(`session expired: ${err.message}`));
  }
}

function router() {
  const hash = location.hash || "#/login";
  if (hash === "#/dashboard") {
    renderDashboard();
  } else {
    mount(renderLogin());
  }
}

window.addEventListener("hashchange", router);
window.addEventListener("DOMContentLoaded", router);
