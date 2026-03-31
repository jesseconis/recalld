const loginPanel = document.getElementById("loginPanel");
const loginForm = document.getElementById("loginForm");
const loginError = document.getElementById("loginError");
const passphraseInput = document.getElementById("passphraseInput");

const appRoot = document.getElementById("app");
const statusLine = document.getElementById("statusLine");
const searchInput = document.getElementById("searchInput");
const resultSummary = document.getElementById("resultSummary");
const pageLabel = document.getElementById("pageLabel");
const prevPageBtn = document.getElementById("prevPageBtn");
const nextPageBtn = document.getElementById("nextPageBtn");
const galleryGrid = document.getElementById("galleryGrid");
const configView = document.getElementById("configView");

const detailFields = document.getElementById("detailFields");
const detailId = document.getElementById("detailId");
const detailTimestamp = document.getElementById("detailTimestamp");
const detailApp = document.getElementById("detailApp");
const detailTitle = document.getElementById("detailTitle");
const detailFilename = document.getElementById("detailFilename");
const detailText = document.getElementById("detailText");

const state = {
  mode: "timeline",
  query: "",
  page: 1,
  perPage: 24,
  totalPages: 0,
  currentItems: [],
  selectedId: null,
  searchDebounce: null,
  activeAbortController: null,
};

loginForm.addEventListener("submit", async (event) => {
  event.preventDefault();
  loginError.textContent = "";

  const passphrase = passphraseInput.value;
  if (!passphrase.trim()) {
    loginError.textContent = "Passphrase is required.";
    return;
  }

  try {
    const response = await fetch("/api/session/login", {
      method: "POST",
      headers: {
        "content-type": "application/json",
      },
      body: JSON.stringify({ passphrase }),
      credentials: "include",
    });

    if (!response.ok) {
      loginError.textContent = "Unlock failed. Check your passphrase.";
      return;
    }

    loginPanel.classList.add("hidden");
    appRoot.classList.remove("hidden");
    passphraseInput.value = "";

    await Promise.all([loadStatus(), loadConfig(), loadData()]);
  } catch (error) {
    loginError.textContent = "Failed to contact server.";
  }
});

searchInput.addEventListener("input", () => {
  const value = searchInput.value.trim();
  state.query = value;
  state.mode = value ? "search" : "timeline";
  state.page = 1;

  if (state.searchDebounce) {
    window.clearTimeout(state.searchDebounce);
  }

  state.searchDebounce = window.setTimeout(() => {
    loadData();
  }, 220);
});

prevPageBtn.addEventListener("click", () => {
  if (state.page <= 1) {
    return;
  }
  state.page -= 1;
  loadData();
});

nextPageBtn.addEventListener("click", () => {
  if (state.totalPages > 0 && state.page >= state.totalPages) {
    return;
  }
  state.page += 1;
  loadData();
});

galleryGrid.addEventListener("click", (event) => {
  const card = event.target.closest("button.card");
  if (!card) {
    return;
  }

  const id = Number(card.dataset.id);
  if (!Number.isFinite(id)) {
    return;
  }

  state.selectedId = id;
  markActiveCard();
  loadDetail(id);

  const url = new URL(window.location.href);
  url.searchParams.set("entry", String(id));
  window.history.replaceState({}, "", url);
});

function markActiveCard() {
  for (const card of galleryGrid.querySelectorAll("button.card")) {
    const id = Number(card.dataset.id);
    card.classList.toggle("active", id === state.selectedId);
  }
}

function buildEndpoint() {
  if (state.mode === "search" && state.query) {
    const params = new URLSearchParams({
      q: state.query,
      page: String(state.page),
      per_page: String(state.perPage),
    });
    return `/api/search?${params.toString()}`;
  }

  const params = new URLSearchParams({
    page: String(state.page),
    per_page: String(state.perPage),
  });
  return `/api/gallery?${params.toString()}`;
}

async function loadData() {
  if (state.activeAbortController) {
    state.activeAbortController.abort();
  }
  const controller = new AbortController();
  state.activeAbortController = controller;

  const endpoint = buildEndpoint();

  try {
    const response = await fetch(endpoint, {
      credentials: "include",
      signal: controller.signal,
    });

    if (response.status === 401) {
      lockUi();
      return;
    }

    if (!response.ok) {
      resultSummary.textContent = "Failed to load screenshots.";
      return;
    }

    const data = await response.json();
    state.page = data.meta.page;
    state.perPage = data.meta.per_page;
    state.totalPages = data.meta.total_pages;
    state.currentItems = data.items;

    renderGallery(data.items);
    renderMeta(data.meta);

    const url = new URL(window.location.href);
    if (state.query) {
      url.searchParams.set("q", state.query);
    } else {
      url.searchParams.delete("q");
    }
    url.searchParams.set("page", String(state.page));
    window.history.replaceState({}, "", url);

    if (state.selectedId && data.items.some((item) => item.id === state.selectedId)) {
      markActiveCard();
      return;
    }

    if (data.items.length > 0) {
      state.selectedId = data.items[0].id;
      markActiveCard();
      await loadDetail(state.selectedId);
    } else {
      clearDetail();
    }
  } catch (error) {
    if (error && error.name === "AbortError") {
      return;
    }
    resultSummary.textContent = "Network error while loading screenshots.";
  }
}

function renderGallery(items) {
  if (!items.length) {
    galleryGrid.innerHTML = `<p class="muted">No screenshots found.</p>`;
    return;
  }

  galleryGrid.innerHTML = items
    .map((item) => {
      const when = formatTimestamp(item.timestamp);
      const encodedFilename = encodeURIComponent(item.screenshot_filename);
      return `
        <button class="card" type="button" data-id="${item.id}">
          <img src="/api/screenshot/${encodedFilename}" loading="lazy" alt="Screenshot ${item.id}" />
          <h3>${escapeHtml(item.title || "(untitled)")}</h3>
          <div class="meta">${escapeHtml(item.app)} · ${escapeHtml(when)}</div>
        </button>
      `;
    })
    .join("");
}

function renderMeta(meta) {
  const modeText = state.mode === "search" ? "Search" : "Gallery";
  resultSummary.textContent = `${modeText}: ${meta.total} result(s)`;
  pageLabel.textContent = `Page ${meta.page}${meta.total_pages ? ` / ${meta.total_pages}` : ""}`;

  prevPageBtn.disabled = meta.page <= 1;
  nextPageBtn.disabled = meta.total_pages === 0 || meta.page >= meta.total_pages;
}

async function loadDetail(id) {
  try {
    const response = await fetch(`/api/entry/${id}`, {
      credentials: "include",
    });

    if (response.status === 401) {
      lockUi();
      return;
    }

    if (!response.ok) {
      return;
    }

    const detail = await response.json();
    detailFields.classList.remove("hidden");
    detailId.textContent = String(detail.id);
    detailTimestamp.textContent = formatTimestamp(detail.timestamp);
    detailApp.textContent = detail.app || "";
    detailTitle.textContent = detail.title || "";
    detailFilename.textContent = detail.screenshot_filename || "";
    detailText.textContent = detail.text || "";
  } catch (_error) {
    // ignore transient detail errors
  }
}

function clearDetail() {
  state.selectedId = null;
  detailFields.classList.add("hidden");
  detailId.textContent = "";
  detailTimestamp.textContent = "";
  detailApp.textContent = "";
  detailTitle.textContent = "";
  detailFilename.textContent = "";
  detailText.textContent = "";
}

async function loadStatus() {
  const response = await fetch("/api/status", {
    credentials: "include",
  });
  if (!response.ok) {
    statusLine.textContent = "Status unavailable";
    return;
  }

  const status = await response.json();
  const lastCapture = status.last_capture_timestamp
    ? formatTimestamp(status.last_capture_timestamp)
    : "none";

  statusLine.textContent = `Backend: ${status.capture_backend} | Entries: ${status.total_entries} | Last: ${lastCapture}`;
}

async function loadConfig() {
  const response = await fetch("/api/config", {
    credentials: "include",
  });
  if (!response.ok) {
    configView.textContent = "Config unavailable";
    return;
  }

  const cfg = await response.json();
  configView.textContent = cfg.config_toml;
}

function lockUi() {
  appRoot.classList.add("hidden");
  loginPanel.classList.remove("hidden");
  loginError.textContent = "Session expired. Unlock again.";
}

function formatTimestamp(epochSeconds) {
  if (!epochSeconds) {
    return "unknown";
  }
  const date = new Date(epochSeconds * 1000);
  if (Number.isNaN(date.getTime())) {
    return `ts=${epochSeconds}`;
  }
  return date.toLocaleString();
}

function escapeHtml(value) {
  return String(value)
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;")
    .replaceAll("'", "&#39;");
}

(function bootFromUrl() {
  const params = new URLSearchParams(window.location.search);
  const q = params.get("q");
  const page = Number(params.get("page"));
  const selected = Number(params.get("entry"));

  if (q) {
    state.query = q;
    state.mode = "search";
    searchInput.value = q;
  }

  if (Number.isFinite(page) && page > 0) {
    state.page = page;
  }

  if (Number.isFinite(selected) && selected > 0) {
    state.selectedId = selected;
  }
})();
