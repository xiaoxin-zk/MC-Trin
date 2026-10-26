/* ============================================================
   DeepCraft Launcher · UI 设计稿交互脚本
   仅用于演示界面切换与状态，不含真实启动逻辑
   ============================================================ */
(function () {
  'use strict';

  const $ = (sel, root = document) => root.querySelector(sel);
  const $$ = (sel, root = document) => Array.from(root.querySelectorAll(sel));

  /* ---------- 视图切换 ---------- */
  const views = $$('.view');
  const navItems = $$('.nav-item');

  function goto(view) {
    const target = views.find((v) => v.dataset.view === view);
    if (!target) return;
    views.forEach((v) => v.classList.toggle('is-active', v === target));
    navItems.forEach((n) => n.classList.toggle('is-active', n.dataset.goto === view));
    $('#main').scrollTop = 0;
    // 视口变化时重算波形等
    window.dispatchEvent(new Event('resize'));
  }

  document.addEventListener('click', (e) => {
    const nav = e.target.closest('[data-goto]');
    if (nav) {
      goto(nav.dataset.goto);
      return;
    }
    const action = e.target.closest('[data-action]');
    if (action) handleAction(action);
  });

  /* ---------- Toast ---------- */
  const stack = $('#toastStack');
  function toast(msg, type = 'ok') {
    const el = document.createElement('div');
    el.className = 'toast';
    const icon = type === 'warn' ? 'i-x' : type === 'info' ? 'i-message' : 'i-check';
    el.innerHTML = `<svg class="i"><use href="#${icon}"/></svg><span></span>`;
    el.querySelector('span').textContent = msg;
    stack.appendChild(el);
    setTimeout(() => {
      el.classList.add('is-out');
      setTimeout(() => el.remove(), 240);
    }, 2400);
  }

  /* ---------- 主题 ---------- */
  function setAccent(name) {
    document.documentElement.dataset.accent = name;
    $$('.accent-dot').forEach((d) => d.classList.toggle('is-active', d.dataset.accent === name));
  }
  function setTheme(mode) {
    const root = document.documentElement;
    if (mode === 'auto') {
      const dark = !window.matchMedia('(prefers-color-scheme: light)').matches;
      root.dataset.theme = dark ? 'dark' : 'light';
    } else {
      root.dataset.theme = mode;
    }
    $$('[data-theme-mode]').forEach((b) => b.classList.toggle('is-active', b.dataset.themeMode === mode));
  }
  setAccent('emerald');
  $$('.accent-dot').forEach((d) =>
    d.addEventListener('click', () => {
      setAccent(d.dataset.accent);
      toast('强调色已切换');
    })
  );
  $$('[data-theme-mode]').forEach((b) =>
    b.addEventListener('click', () => {
      setTheme(b.dataset.themeMode);
      toast('主题模式已切换');
    })
  );

  /* ---------- 弹窗 ---------- */
  const mask = $('#modalMask');
  const modalTitle = $('#modalTitle');
  const modalBody = $('#modalBody');
  const modalFoot = $('#modalFoot');

  function openModal(title, html, footHtml) {
    modalTitle.textContent = title;
    modalBody.innerHTML = html;
    modalFoot.innerHTML =
      footHtml ||
      `<button class="btn btn-ghost" data-action="close-modal">取消</button>
       <button class="btn btn-primary" data-action="confirm-modal">确定</button>`;
    mask.classList.add('is-open');
  }
  function closeModal() {
    mask.classList.remove('is-open');
  }

  const NEW_INSTANCE_HTML = `
    <div class="form-field">
      <label>实例名称</label>
      <input type="text" value="新的冒险" />
      <span class="hint">建议使用能区分整合包的名称</span>
    </div>
    <div class="form-field">
      <label>游戏版本</label>
      <select>
        <option>1.21.4（最新正式版）</option>
        <option>1.20.4</option>
        <option>1.20.1</option>
        <option>1.19.2</option>
        <option>1.8.9</option>
      </select>
    </div>
    <div class="form-field">
      <label>模组加载器</label>
      <div class="opt-grid">
        <button class="opt is-active" data-opt><svg class="i"><use href="#i-zap"/></svg>Fabric</button>
        <button class="opt" data-opt><svg class="i"><use href="#i-wrench"/></svg>NeoForge</button>
        <button class="opt" data-opt><svg class="i"><use href="#i-wrench"/></svg>Forge</button>
        <button class="opt" data-opt><svg class="i"><use href="#i-grid"/></svg>原版</button>
      </div>
    </div>
    <div class="form-field">
      <label>内存分配</label>
      <select>
        <option>4 GB</option>
        <option selected>8 GB（推荐）</option>
        <option>12 GB</option>
        <option>16 GB</option>
      </select>
    </div>`;

  function openCreateRoom() {
    openModal(
      '创建联机房间',
      `
      <div class="form-field">
        <label>房间名称</label>
        <input type="text" value="「星穹冒险」周末开荒队" />
      </div>
      <div class="form-field">
        <label>使用实例</label>
        <select>
          <option>星穹冒险 · Fabric 1.20.4</option>
          <option>机械动力：创世 · NeoForge 1.21.1</option>
          <option>原版生存 1.21.4</option>
        </select>
      </div>
      <div class="form-field">
        <label>人数上限</label>
        <select><option>4 人</option><option selected>8 人</option><option>16 人</option></select>
      </div>
      <div class="form-field">
        <label>房间选项</label>
        <div class="opt-grid">
          <button class="opt is-active" data-opt><svg class="i"><use href="#i-mic"/></svg>开启语音</button>
          <button class="opt is-active" data-opt><svg class="i"><use href="#i-download"/></svg>自动同步模组</button>
          <button class="opt" data-opt><svg class="i"><use href="#i-shield"/></svg>仅好友可加入</button>
          <button class="opt" data-opt><svg class="i"><use href="#i-users"/></svg>允许观战</button>
        </div>
        <span class="hint">房间通过内网穿透创建，无需公网 IP</span>
      </div>`,
      `<button class="btn btn-ghost" data-action="close-modal">取消</button>
       <button class="btn btn-primary" data-action="confirm-modal">创建并进入</button>`
    );
  }

  function openDetail(name) {
    const card = $$('.instance-card').find((c) => c.dataset.instance === name);
    if (card) {
      $$('.instance-card').forEach((c) => c.classList.toggle('is-selected', c === card));
      const cover = $('.instance-cover', card);
      const title = $('.instance-body h3', card).textContent;
      const meta = $('.instance-body p', card).textContent;
      $('#drawerName').textContent = title;
      $('#drawerMeta').textContent = meta;
      const dc = $('#drawerCover');
      dc.className = 'cover ' + (cover ? Array.from(cover.classList).find((c) => c.startsWith('cover-')) : 'cover-a');
      dc.textContent = title.trim().charAt(0);
    }
    openDrawer();
  }

  function openDrawer() {
    $('#drawer').classList.add('is-open');
    $('#drawerMask').classList.add('is-open');
  }
  function closeDrawer() {
    $('#drawer').classList.remove('is-open');
    $('#drawerMask').classList.remove('is-open');
  }

  /* ---------- 启动进度 ---------- */
  const launchMask = $('#launchMask');
  const launchBar = $('#launchBar');
  const launchPct = $('#launchPct');
  const launchStep = $('#launchStep');
  const launchTitle = $('#launchTitle');
  let launchTimer = null;

  const steps = [
    [12, '校验实例文件…'],
    [28, '检查 Java 运行时…'],
    [46, '同步模组与资源…'],
    [66, '构建启动参数…'],
    [84, '加载 Fabric 加载器…'],
    [97, '准备游戏窗口…'],
  ];

  function startLaunch() {
    clearInterval(launchTimer);
    let p = 0;
    let si = 0;
    launchTitle.textContent = '正在启动 星穹冒险';
    launchStep.textContent = steps[0][1];
    launchBar.style.width = '0%';
    launchPct.textContent = '0%';
    launchMask.classList.add('is-open');

    launchTimer = setInterval(() => {
      p = Math.min(100, p + Math.random() * 7 + 3);
      launchBar.style.width = p.toFixed(0) + '%';
      launchPct.textContent = p.toFixed(0) + '%';
      while (si < steps.length && p >= steps[si][0]) {
        launchStep.textContent = steps[si][1];
        si++;
      }
      if (p >= 100) {
        clearInterval(launchTimer);
        launchStep.textContent = '启动完成，游戏窗口已打开';
        setTimeout(() => {
          launchMask.classList.remove('is-open');
          toast('游戏已启动 · Fabric 1.20.4');
        }, 700);
      }
    }, 260);
  }
  function closeLaunch() {
    clearInterval(launchTimer);
    launchMask.classList.remove('is-open');
    toast('已取消启动', 'warn');
  }

  /* ---------- 动作分发 ---------- */
  function handleAction(el) {
    const a = el.dataset.action;
    switch (a) {
      case 'toast':
        toast(el.dataset.msg || '操作已执行');
        break;
      case 'new-instance':
        openModal('新建实例', NEW_INSTANCE_HTML);
        break;
      case 'import':
        toast('已选择整合包文件，正在解析…', 'info');
        break;
      case 'create-room':
        openCreateRoom();
        break;
      case 'join-room':
        toast('正在连接房间并校验模组…', 'info');
        setTimeout(() => toast('已加入房间 · 语音已连接'), 1100);
        break;
      case 'open-detail':
        openDetail(el.closest('.instance-card')?.dataset.instance || '星穹冒险');
        break;
      case 'close-drawer':
        closeDrawer();
        break;
      case 'close-modal':
        closeModal();
        break;
      case 'confirm-modal':
        closeModal();
        toast('已保存配置');
        break;
      case 'launch':
        startLaunch();
        break;
      case 'close-launch':
        closeLaunch();
        break;
      case 'toggle-mic':
        el.classList.toggle('is-on');
        el.classList.toggle('off');
        toast(el.classList.contains('is-on') ? '麦克风已开启' : '麦克风已静音', 'info');
        break;
      case 'install-plugin':
        el.textContent = '已安装';
        el.closest('.plugin-card')?.classList.add('is-installed');
        toast('插件已安装，重启启动器后生效');
        break;
      case 'toggle-plugin':
        el.textContent = el.textContent.trim() === '已安装' ? '已启用' : '已安装';
        toast('插件状态已更新');
        break;
      case 'install-theme': {
        const name = el.dataset.themeName || 'ocean';
        setAccent(name);
        toast('主题已应用：极光主题包');
        break;
      }
      case 'toggle-theme':
        setTheme(document.documentElement.dataset.theme === 'dark' ? 'light' : 'dark');
        toast('已切换明暗主题');
        break;
      case 'ai-send':
        sendAi();
        break;
    }
  }

  /* ---------- AI 对话 ---------- */
  const thread = $('#aiThread');
  const aiInput = $('#aiInput');

  function scrollThread() {
    thread.scrollTop = thread.scrollHeight;
  }

  function sendAi() {
    const text = aiInput.value.trim();
    if (!text) {
      toast('请先输入问题', 'warn');
      return;
    }
    const userMsg = document.createElement('div');
    userMsg.className = 'msg msg-user';
    userMsg.innerHTML = `<div class="msg-body"><p></p></div><span class="avatar sm">林</span>`;
    userMsg.querySelector('p').textContent = text;
    thread.appendChild(userMsg);
    aiInput.value = '';
    aiInput.style.height = 'auto';
    scrollThread();

    const typing = document.createElement('div');
    typing.className = 'msg msg-ai';
    typing.innerHTML = `
      <span class="agent-avatar sm"><svg class="i"><use href="#i-bot"/></svg></span>
      <div class="msg-body">
        <div class="tool-card">
          <div class="tool-head">
            <svg class="i"><use href="#i-terminal"/></svg>
            <span>正在分析上下文…</span>
            <em class="tool-status ok">运行中</em>
          </div>
          <div class="tool-meta">检索实例配置 · 日志索引 · 模组依赖</div>
        </div>
        <p>正在整理诊断结论…</p>
      </div>`;
    thread.appendChild(typing);
    scrollThread();

    setTimeout(() => {
      typing.querySelector('.tool-status').textContent = '完成';
      typing.querySelector('.tool-head span').textContent = '分析完成';
      typing.querySelector('.msg-body > p').innerHTML =
        '已收到你的问题。基于当前实例的配置与最近一次日志，我建议先做一次<strong>完整性校验</strong>，' +
        '并检查 <code>Java</code> 版本是否满足该整合包要求。需要我直接开始诊断吗？';
      scrollThread();
    }, 1400);
  }

  if (aiInput) {
    aiInput.addEventListener('keydown', (e) => {
      if (e.key === 'Enter' && !e.shiftKey) {
        e.preventDefault();
        sendAi();
      }
    });
    aiInput.addEventListener('input', () => {
      aiInput.style.height = 'auto';
      aiInput.style.height = Math.min(120, aiInput.scrollHeight) + 'px';
    });
  }

  $$('#aiSuggest .chip').forEach((chip) =>
    chip.addEventListener('click', () => {
      aiInput.value = chip.textContent;
      sendAi();
    })
  );

  /* ---------- 通用交互 ---------- */
  // chip 单选（按容器分组）
  $$('.chip-row').forEach((row) => {
    $$('.chip', row).forEach((chip) =>
      chip.addEventListener('click', () => {
        $$('.chip', row).forEach((c) => c.classList.remove('is-active'));
        chip.classList.add('is-active');
      })
    );
  });

  // 分段控件
  $$('.seg').forEach((seg) => {
    $$('.seg-btn', seg).forEach((b) =>
      b.addEventListener('click', () => {
        $$('.seg-btn', seg).forEach((x) => x.classList.remove('is-active'));
        b.classList.add('is-active');
      })
    );
  });

  // 选项按钮组（弹窗内）
  document.addEventListener('click', (e) => {
    const opt = e.target.closest('[data-opt]');
    if (opt) opt.classList.toggle('is-active');
  });

  // 标签页（联机大厅）
  const mpTabs = $('#mpTabs');
  if (mpTabs) {
    $$('.tab', mpTabs).forEach((t) =>
      t.addEventListener('click', () => {
        $$('.tab', mpTabs).forEach((x) => x.classList.remove('is-active'));
        t.classList.add('is-active');
        toast(
          t.dataset.tab === 'mine' ? '仅显示你创建的房间' :
          t.dataset.tab === 'friends' ? '仅显示好友房间' : '显示全部推荐房间',
          'info'
        );
      })
    );
  }

  // 抽屉标签
  $$('#drawerTabs .tab').forEach((t) =>
    t.addEventListener('click', () => {
      $$('#drawerTabs .tab').forEach((x) => x.classList.remove('is-active'));
      t.classList.add('is-active');
      $$('.dpanel').forEach((p) => p.classList.toggle('is-active', p.dataset.dpanel === t.dataset.dtab));
    })
  );

  // 设置导航
  $$('#settingsNav .s-nav').forEach((b) =>
    b.addEventListener('click', () => {
      $$('#settingsNav .s-nav').forEach((x) => x.classList.remove('is-active'));
      b.classList.add('is-active');
      $$('.settings-panel').forEach((p) => p.classList.toggle('is-active', p.dataset.panel === b.dataset.panel));
    })
  );

  // 内存滑块
  const memRange = $('#memRange');
  const memVal = $('#memVal');
  if (memRange && memVal) {
    memRange.addEventListener('input', () => {
      memVal.textContent = memRange.value + ' GB';
    });
  }

  // 关闭遮罩
  mask.addEventListener('click', (e) => {
    if (e.target === mask) closeModal();
  });
  $('#drawerMask').addEventListener('click', closeDrawer);
  launchMask.addEventListener('click', (e) => {
    if (e.target === launchMask) closeLaunch();
  });

  // 快捷键
  document.addEventListener('keydown', (e) => {
    if (e.key === 'Escape') {
      closeModal();
      closeDrawer();
      if (launchMask.classList.contains('is-open')) closeLaunch();
    }
    if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 'k') {
      e.preventDefault();
      const input = $('#globalSearch');
      if (input) input.focus();
    }
  });

  // 全局搜索演示
  const globalSearch = $('#globalSearch');
  if (globalSearch) {
    globalSearch.addEventListener('keydown', (e) => {
      if (e.key === 'Enter' && globalSearch.value.trim()) {
        toast(`搜索「${globalSearch.value.trim()}」· 找到 12 条结果`, 'info');
      }
    });
  }

  // 实例卡片点击选中
  $$('.instance-card').forEach((card) => {
    card.addEventListener('click', (e) => {
      if (e.target.closest('[data-action]')) return;
      $$('.instance-card').forEach((c) => c.classList.remove('is-selected'));
      card.classList.add('is-selected');
      openDetail(card.dataset.instance);
    });
  });

  // 启动页最近列表点击启动
  $$('.recent-list li').forEach((li) => {
    li.addEventListener('click', (e) => {
      if (e.target.closest('[data-action]')) return;
      openDetail(li.querySelector('strong').textContent);
    });
  });

  /* ---------- 支持 #view 直达（方便预览/截图） ---------- */
  function applyHash() {
    const v = (location.hash || '').replace('#', '').trim();
    if (v) goto(v);
  }
  window.addEventListener('hashchange', applyHash);
  applyHash();

  /* ---------- 首次提示 ---------- */
  if (!(location.hash || '').trim()) {
    setTimeout(() => toast('设计稿已就绪 · 试试左侧「联机大厅 / AI 助手 / 插件市场」', 'info'), 700);
  }
})();
