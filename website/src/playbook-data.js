const DEFAULT_HERO = {
  kickerZh: 'OpenBitFun 使用指南',
  kickerEn: 'OpenBitFun User Guide',
  titleZh: '顺手用好每一项能力，让 OpenBitFun 真正配合你的节奏',
  titleEn: 'Get comfortable with OpenBitFun—and shape it around how you work',
  subtitleZh:
    '这里不是冷冰冰的参数字典，而是一本随手可查的实用手册。无论是第一次连接模型、在本地或远程服务器上改代码、出门用手机跟进进度，还是搭一套专属 Skills 与定时任务，搜一下就能照着做，也可以复制一句话直接交给智能体。',
  subtitleEn:
    'A practical, human handbook for everyday work in OpenBitFun. Whether you are connecting your first model, coding on a remote SSH server, checking in from your phone, or setting up Skills and scheduled jobs—follow clear steps or copy one sentence for your agent.',
  quickQueriesZh: ['配置模型', '远程工作区', '手机遥控', 'MCP', 'Skills', '定时任务', '深色模式', '快捷键'],
  quickQueriesEn: ['Models', 'Remote SSH', 'Mobile control', 'MCP', 'Skills', 'Scheduled jobs', 'Dark mode', 'Shortcuts'],
};

const DEFAULT_GETTING_STARTED = [
  {
    id: 'connect-models',
    step: 1,
    titleZh: '连接你的主力模型',
    titleEn: 'Connect your first model',
    descriptionZh: '填入 API Key 或登录订阅账号，选好日常对话与快速任务默认使用的模型。',
    descriptionEn: 'Add an API key or subscription account, then pick default models for chat and fast tasks.',
    capabilityId: 'setting.ai.models',
    actionLabelZh: '查看模型配置',
    actionLabelEn: 'Set up models',
  },
  {
    id: 'open-workspace',
    step: 2,
    titleZh: '打开本地或远程项目',
    titleEn: 'Open a local or remote project',
    descriptionZh: '把本地代码目录或 SSH 服务器变成工作区，文件树、终端和会话上下文自动对齐。',
    descriptionEn: 'Open a local folder or SSH host so files, terminal, and chat share one workspace context.',
    capabilityId: 'feature.projects',
    actionLabelZh: '了解项目工作区',
    actionLabelEn: 'Explore workspaces',
  },
  {
    id: 'start-session',
    step: 3,
    titleZh: '发起第一次 AI 协作',
    titleEn: 'Start your first AI session',
    descriptionZh: '直接说出目标、拖入文件或截图，随时查看代码差异、审批权限或中途调整方向。',
    descriptionEn: 'Describe your goal, drop in files or screenshots, review diffs, and steer work live.',
    capabilityId: 'feature.ai-assistant',
    actionLabelZh: '阅读会话指南',
    actionLabelEn: 'Read session guide',
  },
  {
    id: 'extend-workflow',
    step: 4,
    titleZh: '装上趁手工具与跨端遥控',
    titleEn: 'Add Skills, MCP & mobile access',
    descriptionZh: '按需接入常用 Skills、MCP 服务与定时任务，或绑定手机随时查看和回复长任务。',
    descriptionEn: 'Plug in Skills, MCP servers, and schedules, or pair your phone to follow long tasks anywhere.',
    capabilityId: 'feature.remote-connect',
    actionLabelZh: '连接多设备',
    actionLabelEn: 'Connect devices',
  },
];

const DEFAULT_SCENARIOS = [
  {
    id: 'daily-coding',
    titleZh: '日常改代码与放心审阅',
    titleEn: 'Everyday coding & confident review',
    descriptionZh: '让智能体写代码、跑终端命令验证结果，再逐处查看 Diff、按轮次回滚或协助评审 PR。',
    descriptionEn: 'Let the agent edit files and run terminal checks, then inspect diffs, roll back turns, or review PRs.',
    capabilityIds: ['feature.ai-assistant', 'feature.files-editor', 'feature.terminal', 'feature.code-review'],
  },
  {
    id: 'remote-ssh-dev',
    titleZh: '连上远程服务器无感开发',
    titleEn: 'Develop directly on remote SSH servers',
    descriptionZh: '代码留在远端 Linux 开发机或容器里，本地照常浏览文件、开终端、转发端口与驱动智能体。',
    descriptionEn: 'Keep code and builds on a remote Linux host while browsing files, terminals, and ports from your desk.',
    capabilityIds: ['feature.remote-workspaces', 'setting.application.terminal', 'setting.tools.acp'],
  },
  {
    id: 'mobile-anywhere',
    titleZh: '离开电脑也能用手机跟进',
    titleEn: 'Follow & steer work from your phone',
    descriptionZh: '通过手机网页或飞书、Telegram、微信机器人接收进度、审批权限，甚至向另一台主机派发后台任务。',
    descriptionEn: 'Answer permission prompts, read live progress, or dispatch background jobs from mobile web or IM bots.',
    capabilityIds: ['feature.remote-connect', 'feature.detached-dispatch', 'setting.tools.execution'],
  },
  {
    id: 'skills-automation',
    titleZh: '把团队经验沉淀成自动化',
    titleEn: 'Turn repeatable routines into Skills & jobs',
    descriptionZh: '用 Skills 固化常用工作流，用 MCP 连接外部系统，再配上定时任务与事件 Hooks 自动运转。',
    descriptionEn: 'Capture team workflows as Skills, connect tools via MCP, and run recurring tasks or Hooks on autopilot.',
    capabilityIds: ['feature.skills', 'feature.tasks-automation', 'setting.tools.mcp', 'setting.tools.automation'],
  },
  {
    id: 'migrate-setup',
    titleZh: '把已有 AI 工具配置直接带过来',
    titleEn: 'Bring your Claude Code or Codex setup over',
    descriptionZh: '一键扫描并导入电脑上已有的 Skills、MCP、Hooks 与历史会话，不用从零重新配置。',
    descriptionEn: 'Discover and import supported Skills, MCP servers, Hooks, and sessions you already use elsewhere.',
    capabilityIds: ['feature.ecosystem-compatibility', 'setting.ai.models', 'setting.ai.memory'],
  },
];

export const DEFAULT_DIRECTORY_GROUPS = [
  {
    id: 'feature-conversations',
    kind: 'feature',
    order: 10,
    titleZh: '对话协作与智能体',
    titleEn: 'Conversations & Agents',
    descriptionZh: '围绕项目展开对话、分工给多角色智能体，或打造长期陪伴你的个人助理。',
    descriptionEn: 'Work in project sessions, delegate to specialized agents, and shape long-term personal assistants.',
  },
  {
    id: 'feature-coding',
    kind: 'feature',
    order: 20,
    titleZh: '项目、编码与代码审阅',
    titleEn: 'Projects, Coding & Review',
    descriptionZh: '管理工作区、浏览和修改代码、对比每次改动差异、操作终端与评审 Pull Request。',
    descriptionEn: 'Manage workspaces, edit files, inspect diffs, run terminals, and review pull requests.',
  },
  {
    id: 'feature-remote',
    kind: 'feature',
    order: 30,
    titleZh: '远程开发、多设备与网页发布',
    titleEn: 'Remote Work, Devices & Publishing',
    descriptionZh: '连接 SSH 远程主机、用手机或聊天机器人遥控电脑、跨设备派发后台任务，以及一键发布网页。',
    descriptionEn: 'Work over SSH, control sessions from mobile or IM bots, dispatch jobs across hosts, and publish web pages.',
  },
  {
    id: 'feature-extensions',
    kind: 'feature',
    order: 40,
    titleZh: '技能扩展、小应用与定时任务',
    titleEn: 'Skills, Mini Apps & Automation',
    descriptionZh: '安装或编写 Skills、打开交互式小应用、安排定时任务，并回顾你的协作洞察。',
    descriptionEn: 'Add reusable Skills, launch interactive Mini Apps, schedule recurring tasks, and review work insights.',
  },
  {
    id: 'feature-multimodal',
    kind: 'feature',
    order: 50,
    titleZh: '浏览器、桌面操作与语音伙伴',
    titleEn: 'Browser, Desktop Control & Voice',
    descriptionZh: '内置网页预览与拾取元素、授权操作外部浏览器或桌面界面、语音通话，以及桌面伙伴与小卡片。',
    descriptionEn: 'Preview and inspect pages, automate browsers or desktop UI with approval, talk by voice, and use desktop companions.',
  },
  {
    id: 'setting-ai',
    kind: 'setting',
    order: 60,
    titleZh: '模型、记忆与执行权限',
    titleEn: 'Models, Memory & Permissions',
    descriptionZh: '配置模型供应商与默认路由、管理会话默认行为与长期记忆，以及把控工具审批与超时边界。',
    descriptionEn: 'Configure AI providers, default models, session memory, approval rules, and execution limits.',
  },
  {
    id: 'setting-development',
    kind: 'setting',
    order: 70,
    titleZh: '编辑器、终端与工作区 Git',
    titleEn: 'Editor, Terminal & Worktrees',
    descriptionZh: '调整代码缩进和字号、选择默认终端 Shell，以及配置隔离分支开发的 Git Worktree。',
    descriptionEn: 'Tune code editor preferences, pick your default terminal shell, and manage Git worktrees.',
  },
  {
    id: 'setting-integrations',
    kind: 'setting',
    order: 80,
    titleZh: 'MCP、自动化与外部生态接入',
    titleEn: 'MCP, Hooks & External Integrations',
    descriptionZh: '接入 MCP 工具服务、联网搜索、事件 Hooks、ACP 外部智能体，或从其他 AI 工具导入配置。',
    descriptionEn: 'Connect MCP servers, web search, event Hooks, ACP agents, Browser/Desktop permissions, and External Import.',
  },
  {
    id: 'setting-interface',
    kind: 'setting',
    order: 90,
    titleZh: '外观主题、快捷键与交互习惯',
    titleEn: 'Appearance, Shortcuts & Interaction',
    descriptionZh: '切换浅色/深色与内置主题皮肤、界面语言、发送消息快捷键、语音转文字与桌面宠物形象。',
    descriptionEn: 'Switch themes and UI language, customize keyboard shortcuts, tune voice input, and personalize your pet.',
  },
  {
    id: 'setting-data',
    kind: 'setting',
    order: 100,
    titleZh: '用量统计、会话归档与排障',
    titleEn: 'Usage, Archives & Diagnostics',
    descriptionZh: '查看模型调用与 Token 开销、找回或清理归档会话，以及导出排障日志。',
    descriptionEn: 'Check token and request usage, restore or clean up archived sessions, and export diagnostic logs.',
  },
];

const DEFAULT_CAPABILITY_META = {
  'feature.ai-assistant': {
    directoryGroupId: 'feature-conversations',
    order: 1,
    shortcut: '⌘N',
    entryPathZh: '左侧导航栏 → 会话（主工作台）',
    entryPathEn: 'Left Sidebar → Sessions (Main Workspace)',
    relatedIds: ['setting.ai.models', 'setting.ai.memory', 'setting.tools.execution', 'feature.files-editor'],
  },
  'feature.agents': {
    directoryGroupId: 'feature-conversations',
    order: 2,
    entryPathZh: '左侧导航栏 → 智能体',
    entryPathEn: 'Left Sidebar → Agents',
    relatedIds: ['feature.ai-assistant', 'feature.skills', 'setting.tools.execution'],
  },
  'feature.personal-assistants': {
    directoryGroupId: 'feature-conversations',
    order: 3,
    entryPathZh: '左侧导航栏 → 个人助理空间',
    entryPathEn: 'Left Sidebar → Personal Assistants',
    relatedIds: ['feature.ai-assistant', 'feature.tasks-automation', 'setting.ai.memory'],
  },
  'feature.generative-ui': {
    directoryGroupId: 'feature-conversations',
    order: 4,
    entryPathZh: 'AI 会话流中自动内嵌展示（向智能体提出可视化需求即可触发）',
    entryPathEn: 'Rendered inline in AI sessions when you ask for interactive visuals',
    relatedIds: ['feature.ai-assistant', 'feature.miniapps', 'feature.pages'],
  },
  'feature.projects': {
    directoryGroupId: 'feature-coding',
    order: 1,
    shortcut: '⌘O',
    entryPathZh: '顶部工作区切换器 → 打开项目 / 新建项目',
    entryPathEn: 'Top Workspace Switcher → Open Folder / New Project',
    relatedIds: ['feature.files-editor', 'feature.remote-workspaces', 'setting.workspace.session'],
  },
  'feature.files-editor': {
    directoryGroupId: 'feature-coding',
    order: 2,
    shortcut: '⌘P',
    entryPathZh: '工作区侧栏 → 文件树与代码编辑器',
    entryPathEn: 'Workspace Panel → File Tree & Code Editor',
    relatedIds: ['setting.application.development', 'feature.code-review', 'feature.projects'],
  },
  'feature.terminal': {
    directoryGroupId: 'feature-coding',
    order: 3,
    shortcut: '⌃`',
    entryPathZh: '工作区面板 → 终端',
    entryPathEn: 'Workspace Panel → Terminal',
    relatedIds: ['setting.application.terminal', 'feature.remote-workspaces', 'feature.ai-assistant'],
  },
  'feature.git': {
    directoryGroupId: 'feature-coding',
    order: 4,
    entryPathZh: '对话中直接让智能体处理 Git，或前往 设置 → 工作区与 Git',
    entryPathEn: 'Ask your agent in chat, or open Settings → Workspace & Git',
    relatedIds: ['feature.code-review', 'setting.workspace.worktrees', 'feature.files-editor'],
  },
  'feature.code-review': {
    directoryGroupId: 'feature-coding',
    order: 5,
    entryPathZh: '工作区文件与变更面板 → 改动审阅 / Pull Request',
    entryPathEn: 'Files & Changes Panel → Diff Review / Pull Requests',
    relatedIds: ['feature.files-editor', 'feature.git', 'setting.tools.execution'],
  },
  'feature.remote-workspaces': {
    directoryGroupId: 'feature-remote',
    order: 1,
    entryPathZh: '顶部工作区切换器 → 打开远程工作区 (SSH)',
    entryPathEn: 'Workspace Switcher → Open Remote Workspace (SSH)',
    relatedIds: ['feature.projects', 'feature.terminal', 'setting.tools.acp'],
  },
  'feature.remote-connect': {
    directoryGroupId: 'feature-remote',
    order: 2,
    entryPathZh: '左下角连接入口 → Remote Connect 与多设备控制',
    entryPathEn: 'Bottom-left Connect → Remote Connect & Devices',
    relatedIds: ['feature.detached-dispatch', 'feature.remote-workspaces', 'setting.tools.execution'],
  },
  'feature.detached-dispatch': {
    directoryGroupId: 'feature-remote',
    order: 3,
    entryPathZh: '左下角连接入口 → Remote Connect → 账号设备与远程派发',
    entryPathEn: 'Bottom-left Connect → Remote Connect → Account Devices & Dispatch',
    relatedIds: ['feature.remote-connect', 'feature.tasks-automation', 'setting.workspace.worktrees'],
  },
  'feature.pages': {
    directoryGroupId: 'feature-remote',
    order: 4,
    entryPathZh: '左侧导航栏 → Pages（或对话中让智能体帮你部署网页）',
    entryPathEn: 'Left Sidebar → Pages (or ask your agent to publish a page)',
    relatedIds: ['feature.miniapps', 'feature.generative-ui', 'feature.browser'],
  },
  'feature.skills': {
    directoryGroupId: 'feature-extensions',
    order: 1,
    entryPathZh: '左侧导航栏 → Skills 技能',
    entryPathEn: 'Left Sidebar → Skills',
    relatedIds: ['feature.agents', 'setting.tools.mcp', 'feature.ecosystem-compatibility'],
  },
  'feature.miniapps': {
    directoryGroupId: 'feature-extensions',
    order: 2,
    entryPathZh: '左侧导航栏 → Mini Apps 小应用',
    entryPathEn: 'Left Sidebar → Mini Apps',
    relatedIds: ['feature.generative-ui', 'feature.pages', 'feature.skills'],
  },
  'feature.tasks-automation': {
    directoryGroupId: 'feature-extensions',
    order: 3,
    entryPathZh: '左侧导航栏 → 任务与定时任务',
    entryPathEn: 'Left Sidebar → Tasks & Scheduled Jobs',
    relatedIds: ['setting.tools.automation', 'feature.personal-assistants', 'feature.detached-dispatch'],
  },
  'feature.insights': {
    directoryGroupId: 'feature-extensions',
    order: 4,
    entryPathZh: '左侧导航栏 → Insights 工作洞察',
    entryPathEn: 'Left Sidebar → Insights',
    relatedIds: ['setting.data.usage', 'feature.ai-assistant', 'setting.ai.memory'],
  },
  'feature.browser': {
    directoryGroupId: 'feature-multimodal',
    order: 1,
    entryPathZh: '工作区面板 → 内置浏览器',
    entryPathEn: 'Workspace Panel → Built-in Browser',
    relatedIds: ['feature.computer-use', 'setting.tools.browser-control', 'feature.pages'],
  },
  'feature.computer-use': {
    directoryGroupId: 'feature-multimodal',
    order: 2,
    entryPathZh: '设置 (⌘,) → 工具与集成 → 浏览器与桌面控制',
    entryPathEn: 'Settings (⌘,) → Tools & Integrations → Browser & Desktop Control',
    relatedIds: ['setting.tools.desktop-control', 'setting.tools.browser-control', 'feature.browser'],
  },
  'feature.voice-input': {
    directoryGroupId: 'feature-multimodal',
    order: 3,
    entryPathZh: '会话输入框麦克风图标，或 设置 (⌘,) → 应用与界面 → 输入与交互 → 语音',
    entryPathEn: 'Chat Composer Mic Button, or Settings (⌘,) → Input & Interaction → Voice',
    relatedIds: ['setting.application.input', 'setting.application.pet', 'feature.desktop-pet'],
  },
  'feature.desktop-pet': {
    directoryGroupId: 'feature-multimodal',
    order: 4,
    entryPathZh: '设置 (⌘,) → 应用与界面 → 宠物与助手 → 宠物',
    entryPathEn: 'Settings (⌘,) → Application & Interface → Pet & assistant → Pet',
    relatedIds: ['setting.application.pet', 'feature.desktop-cards', 'feature.voice-input'],
  },
  'feature.desktop-cards': {
    directoryGroupId: 'feature-multimodal',
    order: 5,
    entryPathZh: '设置 (⌘,) → 应用与界面 → 宠物与助手 → 小卡片',
    entryPathEn: 'Settings (⌘,) → Application & Interface → Pet & assistant → Desktop cards',
    relatedIds: ['feature.desktop-pet', 'setting.application.pet', 'feature.tasks-automation'],
  },
  'setting.ai.models': {
    directoryGroupId: 'setting-ai',
    order: 1,
    shortcut: '⌘,',
    entryPathZh: '设置 (⌘,) → 模型与智能体 → 模型',
    entryPathEn: 'Settings (⌘,) → Models & Agents → Models',
    relatedIds: ['feature.ai-assistant', 'setting.data.usage', 'setting.tools.execution'],
  },
  'setting.workspace.session': {
    directoryGroupId: 'setting-ai',
    order: 2,
    entryPathZh: '设置 (⌘,) → 模型与智能体 → 会话与记忆 → 会话默认行为',
    entryPathEn: 'Settings (⌘,) → Models & Agents → Sessions & Memory → Session Defaults',
    relatedIds: ['feature.ai-assistant', 'setting.ai.memory', 'feature.projects'],
  },
  'setting.ai.memory': {
    directoryGroupId: 'setting-ai',
    order: 3,
    entryPathZh: '设置 (⌘,) → 模型与智能体 → 会话与记忆 → 记忆',
    entryPathEn: 'Settings (⌘,) → Models & Agents → Sessions & Memory → Memory',
    relatedIds: ['setting.workspace.session', 'feature.ai-assistant', 'feature.personal-assistants'],
  },
  'setting.tools.execution': {
    directoryGroupId: 'setting-ai',
    order: 4,
    entryPathZh: '设置 (⌘,) → 模型与智能体 → 执行与协作 / 权限规则',
    entryPathEn: 'Settings (⌘,) → Models & Agents → Execution & Collaboration / Permission Rules',
    relatedIds: ['feature.ai-assistant', 'feature.agents', 'setting.tools.mcp'],
  },
  'setting.application.development': {
    directoryGroupId: 'setting-development',
    order: 1,
    entryPathZh: '设置 (⌘,) → 编程与开发 → 编辑器',
    entryPathEn: 'Settings (⌘,) → Programming & Development → Editor',
    relatedIds: ['feature.files-editor', 'setting.application.appearance', 'setting.application.shortcuts'],
  },
  'setting.application.terminal': {
    directoryGroupId: 'setting-development',
    order: 2,
    entryPathZh: '设置 (⌘,) → 编程与开发 → 终端',
    entryPathEn: 'Settings (⌘,) → Programming & Development → Terminal',
    relatedIds: ['feature.terminal', 'feature.remote-workspaces'],
  },
  'setting.workspace.worktrees': {
    directoryGroupId: 'setting-development',
    order: 3,
    entryPathZh: '设置 (⌘,) → 编程与开发 → 工作区与 Git → Worktrees',
    entryPathEn: 'Settings (⌘,) → Programming & Development → Workspace & Git → Worktrees',
    relatedIds: ['feature.git', 'feature.projects', 'feature.detached-dispatch'],
  },
  'setting.tools.mcp': {
    directoryGroupId: 'setting-integrations',
    order: 1,
    entryPathZh: '设置 (⌘,) → 工具与集成 → MCP',
    entryPathEn: 'Settings (⌘,) → Tools & Integrations → MCP',
    relatedIds: ['feature.skills', 'feature.ecosystem-compatibility', 'setting.tools.execution'],
  },
  'setting.tools.automation': {
    directoryGroupId: 'setting-integrations',
    order: 2,
    entryPathZh: '设置 (⌘,) → 工具与集成 → 自动化（快捷动作 / 事件 Hooks）',
    entryPathEn: 'Settings (⌘,) → Tools & Integrations → Automation (Quick Actions / Event Hooks)',
    relatedIds: ['feature.tasks-automation', 'feature.ecosystem-compatibility', 'feature.skills'],
  },
  'setting.tools.web-search': {
    directoryGroupId: 'setting-integrations',
    order: 3,
    entryPathZh: '设置 (⌘,) → 工具与集成 → 网页搜索',
    entryPathEn: 'Settings (⌘,) → Tools & Integrations → Web Search',
    relatedIds: ['feature.ai-assistant', 'feature.browser', 'setting.tools.mcp'],
  },
  'setting.tools.acp': {
    directoryGroupId: 'setting-integrations',
    order: 4,
    entryPathZh: '设置 (⌘,) → 工具与集成 → ACP',
    entryPathEn: 'Settings (⌘,) → Tools & Integrations → ACP',
    relatedIds: ['feature.agents', 'feature.remote-workspaces', 'feature.ecosystem-compatibility'],
  },
  'feature.ecosystem-compatibility': {
    directoryGroupId: 'setting-integrations',
    order: 5,
    entryPathZh: '设置 (⌘,) → 工具与集成 → 外部导入',
    entryPathEn: 'Settings (⌘,) → Tools & Integrations → External import',
    relatedIds: ['feature.skills', 'setting.tools.mcp', 'setting.tools.automation'],
  },
  'setting.tools.browser-control': {
    directoryGroupId: 'setting-integrations',
    order: 6,
    entryPathZh: '设置 (⌘,) → 工具与集成 → 浏览器与桌面控制',
    entryPathEn: 'Settings (⌘,) → Tools & Integrations → Browser & Desktop Control',
    relatedIds: ['feature.browser', 'feature.computer-use', 'setting.tools.desktop-control'],
  },
  'setting.tools.desktop-control': {
    directoryGroupId: 'setting-integrations',
    order: 7,
    entryPathZh: '设置 (⌘,) → 工具与集成 → 浏览器与桌面控制',
    entryPathEn: 'Settings (⌘,) → Tools & Integrations → Browser & Desktop Control',
    relatedIds: ['feature.computer-use', 'setting.tools.browser-control', 'setting.tools.execution'],
  },
  'setting.application.general': {
    directoryGroupId: 'setting-interface',
    order: 1,
    shortcut: '⌘,',
    entryPathZh: '设置 (⌘,) → 应用与界面 → 通用',
    entryPathEn: 'Settings (⌘,) → Application & Interface → General',
    relatedIds: ['setting.application.appearance', 'setting.application.input', 'setting.data.diagnostics'],
  },
  'setting.application.appearance': {
    directoryGroupId: 'setting-interface',
    order: 2,
    entryPathZh: '设置 (⌘,) → 应用与界面 → 外观',
    entryPathEn: 'Settings (⌘,) → Application & Interface → Appearance',
    relatedIds: ['setting.application.general', 'setting.application.development', 'setting.application.pet'],
  },
  'setting.application.input': {
    directoryGroupId: 'setting-interface',
    order: 3,
    entryPathZh: '设置 (⌘,) → 应用与界面 → 输入与交互',
    entryPathEn: 'Settings (⌘,) → Application & Interface → Input & Interaction',
    relatedIds: ['setting.application.shortcuts', 'feature.voice-input', 'setting.application.general'],
  },
  'setting.application.shortcuts': {
    directoryGroupId: 'setting-interface',
    order: 4,
    shortcut: '⌘K',
    entryPathZh: '设置 (⌘,) → 应用与界面 → 输入与交互 → 快捷键',
    entryPathEn: 'Settings (⌘,) → Application & Interface → Input & Interaction → Keyboard Shortcuts',
    relatedIds: ['setting.application.input', 'feature.files-editor', 'feature.terminal'],
  },
  'setting.application.pet': {
    directoryGroupId: 'setting-interface',
    order: 5,
    entryPathZh: '设置 (⌘,) → 应用与界面 → 宠物与助手',
    entryPathEn: 'Settings (⌘,) → Application & Interface → Pet & assistant',
    relatedIds: ['feature.desktop-pet', 'feature.desktop-cards', 'feature.voice-input'],
  },
  'setting.data.usage': {
    directoryGroupId: 'setting-data',
    order: 1,
    entryPathZh: '设置 (⌘,) → 数据与维护 → 调用统计',
    entryPathEn: 'Settings (⌘,) → Data & Maintenance → Call Statistics',
    relatedIds: ['setting.ai.models', 'feature.insights', 'feature.ai-assistant'],
  },
  'setting.data.archived': {
    directoryGroupId: 'setting-data',
    order: 2,
    entryPathZh: '设置 (⌘,) → 数据与维护 → 已归档会话',
    entryPathEn: 'Settings (⌘,) → Data & Maintenance → Archived Sessions',
    relatedIds: ['feature.ai-assistant', 'setting.workspace.session'],
  },
  'setting.data.diagnostics': {
    directoryGroupId: 'setting-data',
    order: 3,
    entryPathZh: '设置 (⌘,) → 数据与维护 → 日志与诊断',
    entryPathEn: 'Settings (⌘,) → Data & Maintenance → Logs & Diagnostics',
    relatedIds: ['setting.application.general', 'setting.tools.mcp', 'feature.remote-workspaces'],
  },
};

const CATEGORY_TO_FALLBACK_GROUP = {
  'feature:assistant': 'feature-conversations',
  'feature:workspace': 'feature-coding',
  'feature:connect': 'feature-remote',
  'feature:automation': 'feature-extensions',
  'feature:application': 'feature-multimodal',
  'feature:data': 'feature-extensions',
  'setting:assistant': 'setting-ai',
  'setting:workspace': 'setting-development',
  'setting:automation': 'setting-integrations',
  'setting:connect': 'setting-integrations',
  'setting:application': 'setting-interface',
  'setting:data': 'setting-data',
};

export function deriveFallbackEntryPath(capability, language = 'zh') {
  const destination = capability.destination ?? {};
  if (destination.kind === 'settings') {
    const page = destination.pageId ?? '';
    return language === 'zh'
      ? `设置 (⌘,) → 搜索“${capability.titleZh}”即可直达（${page}）`
      : `Settings (⌘,) → Search “${capability.titleEn}” to jump straight there (${page})`;
  }
  if (destination.kind === 'scene') {
    return language === 'zh'
      ? `左侧导航栏 → ${capability.titleZh}（也可按 ⌘K 搜索打开）`
      : `Left Sidebar → ${capability.titleEn} (or press ⌘K to open)`;
  }
  if (destination.kind === 'action') {
    return language === 'zh'
      ? `工作区导航栏或全局搜索 (⌘K) → 输入“${capability.titleZh}”打开`
      : `Workspace navigation or Global Search (⌘K) → Type “${capability.titleEn}”`;
  }
  return language === 'zh'
    ? `按 ⌘K 打开全局搜索，输入“${capability.titleZh}”即可定位`
    : `Press ⌘K for Global Search and type “${capability.titleEn}”`;
}

function compareDirectoryGroups(left, right) {
  const kindOrder = left.kind === right.kind ? 0 : left.kind === 'feature' ? -1 : 1;
  return kindOrder || left.order - right.order || left.id.localeCompare(right.id);
}

export function normalizePlaybookCatalog(rawCatalog) {
  const rawPlaybook = rawCatalog.playbook ?? {};
  const capabilityIdSet = new Set((rawCatalog.capabilities ?? []).map(({ id }) => id));

  const hero = {
    kickerZh: rawPlaybook.hero?.kickerZh || DEFAULT_HERO.kickerZh,
    kickerEn: rawPlaybook.hero?.kickerEn || DEFAULT_HERO.kickerEn,
    titleZh: rawPlaybook.hero?.titleZh || DEFAULT_HERO.titleZh,
    titleEn: rawPlaybook.hero?.titleEn || DEFAULT_HERO.titleEn,
    subtitleZh: rawPlaybook.hero?.subtitleZh || DEFAULT_HERO.subtitleZh,
    subtitleEn: rawPlaybook.hero?.subtitleEn || DEFAULT_HERO.subtitleEn,
    quickQueriesZh: Array.isArray(rawPlaybook.hero?.quickQueriesZh) && rawPlaybook.hero.quickQueriesZh.length
      ? rawPlaybook.hero.quickQueriesZh
      : DEFAULT_HERO.quickQueriesZh,
    quickQueriesEn: Array.isArray(rawPlaybook.hero?.quickQueriesEn) && rawPlaybook.hero.quickQueriesEn.length
      ? rawPlaybook.hero.quickQueriesEn
      : DEFAULT_HERO.quickQueriesEn,
  };

  const rawGroups = Array.isArray(rawPlaybook.directoryGroups) && rawPlaybook.directoryGroups.length
    ? rawPlaybook.directoryGroups
    : DEFAULT_DIRECTORY_GROUPS;

  const directoryGroups = [...rawGroups]
    .map((group, index) => ({
      ...group,
      order: Number.isFinite(group.order) ? group.order : (index + 1) * 10,
    }))
    .sort(compareDirectoryGroups);

  const groupIds = new Set(directoryGroups.map(({ id }) => id));
  const fallbackGroupsUsed = new Map();

  const capabilities = (rawCatalog.capabilities ?? []).map((capability, index) => {
    const defaults = DEFAULT_CAPABILITY_META[capability.id] ?? {};
    let directoryGroupId = capability.directoryGroupId || defaults.directoryGroupId;
    if (!directoryGroupId || !groupIds.has(directoryGroupId)) {
      const candidate = CATEGORY_TO_FALLBACK_GROUP[`${capability.kind}:${capability.categoryId}`];
      if (candidate && groupIds.has(candidate)) {
        directoryGroupId = candidate;
      } else {
        const sameKindGroup = directoryGroups.find((group) => group.kind === capability.kind);
        if (sameKindGroup) {
          directoryGroupId = sameKindGroup.id;
        } else {
          const syntheticId = `fallback-${capability.kind}-${capability.categoryId}`;
          if (!fallbackGroupsUsed.has(syntheticId)) {
            const cat = rawCatalog.categories?.[capability.categoryId];
            fallbackGroupsUsed.set(syntheticId, {
              id: syntheticId,
              kind: capability.kind,
              order: 900 + fallbackGroupsUsed.size,
              titleZh: cat?.titleZh ?? (capability.kind === 'feature' ? '更多功能' : '更多设置'),
              titleEn: cat?.titleEn ?? (capability.kind === 'feature' ? 'More features' : 'More settings'),
              descriptionZh: cat?.descriptionZh ?? '',
              descriptionEn: cat?.descriptionEn ?? '',
            });
          }
          directoryGroupId = syntheticId;
        }
      }
    }

    const order = Number.isFinite(capability.order)
      ? capability.order
      : Number.isFinite(defaults.order)
        ? defaults.order
        : 100 + index;

    const entryPathZh = capability.entryPathZh || defaults.entryPathZh || deriveFallbackEntryPath(capability, 'zh');
    const entryPathEn = capability.entryPathEn || defaults.entryPathEn || deriveFallbackEntryPath(capability, 'en');
    const shortcut = capability.shortcut || defaults.shortcut || '';

    const rawRelated = Array.isArray(capability.relatedIds) && capability.relatedIds.length
      ? capability.relatedIds
      : (defaults.relatedIds ?? []);
    const relatedIds = rawRelated.filter((id) => id !== capability.id && capabilityIdSet.has(id));

    return {
      ...capability,
      directoryGroupId,
      order,
      entryPathZh,
      entryPathEn,
      shortcut,
      highlightsZh: Array.isArray(capability.highlightsZh) ? capability.highlightsZh : [],
      highlightsEn: Array.isArray(capability.highlightsEn) ? capability.highlightsEn : [],
      scenariosZh: Array.isArray(capability.scenariosZh) ? capability.scenariosZh : [],
      scenariosEn: Array.isArray(capability.scenariosEn) ? capability.scenariosEn : [],
      tipsZh: Array.isArray(capability.tipsZh) ? capability.tipsZh : [],
      tipsEn: Array.isArray(capability.tipsEn) ? capability.tipsEn : [],
      relatedIds,
    };
  });

  const allGroups = [...directoryGroups, ...fallbackGroupsUsed.values()]
    .filter((group) => capabilities.some((capability) => capability.directoryGroupId === group.id))
    .sort(compareDirectoryGroups);

  const groupById = Object.fromEntries(allGroups.map((group) => [group.id, group]));

  const gettingStartedSource = Array.isArray(rawPlaybook.gettingStarted) && rawPlaybook.gettingStarted.length
    ? rawPlaybook.gettingStarted
    : DEFAULT_GETTING_STARTED;
  const gettingStarted = gettingStartedSource
    .filter((item) => capabilityIdSet.has(item.capabilityId))
    .map((item, index) => ({
      ...item,
      step: Number.isFinite(item.step) ? item.step : index + 1,
    }));

  const scenariosSource = Array.isArray(rawPlaybook.scenarios) && rawPlaybook.scenarios.length
    ? rawPlaybook.scenarios
    : DEFAULT_SCENARIOS;
  const scenarios = scenariosSource
    .map((scenario) => ({
      ...scenario,
      capabilityIds: (scenario.capabilityIds ?? []).filter((id) => capabilityIdSet.has(id)),
    }))
    .filter((scenario) => scenario.capabilityIds.length > 0);

  const orderedCapabilities = allGroups.flatMap((group) =>
    capabilities
      .filter((capability) => capability.directoryGroupId === group.id)
      .sort((left, right) => left.order - right.order || left.id.localeCompare(right.id)),
  );

  return {
    ...rawCatalog,
    capabilities: orderedCapabilities,
    playbook: {
      hero,
      gettingStarted,
      scenarios,
      directoryGroups: allGroups,
      groupById,
    },
  };
}

export function resolveRelatedCapabilities(catalog, capability, maxCount = 4) {
  const byId = new Map(catalog.capabilities.map((entry) => [entry.id, entry]));
  const chosen = [];
  const seen = new Set([capability.id]);

  for (const relatedId of capability.relatedIds ?? []) {
    const target = byId.get(relatedId);
    if (target && !seen.has(target.id)) {
      seen.add(target.id);
      chosen.push(target);
      if (chosen.length >= maxCount) return chosen;
    }
  }

  // Pair cross-kind entries in the same semantic category first (e.g. feature <-> setting),
  // then siblings in the same Functional Directory group.
  const crossKindSameCategory = catalog.capabilities.filter(
    (entry) => !seen.has(entry.id)
      && entry.categoryId === capability.categoryId
      && entry.kind !== capability.kind,
  );
  const sameGroup = catalog.capabilities.filter(
    (entry) => !seen.has(entry.id) && entry.directoryGroupId === capability.directoryGroupId,
  );

  for (const candidate of [...crossKindSameCategory, ...sameGroup]) {
    if (!seen.has(candidate.id)) {
      seen.add(candidate.id);
      chosen.push(candidate);
      if (chosen.length >= maxCount) break;
    }
  }

  return chosen;
}

export function resolveAdjacentCapabilities(catalog, capabilityId) {
  const list = catalog.capabilities;
  const index = list.findIndex(({ id }) => id === capabilityId);
  if (index < 0) return { previous: null, next: null };
  return {
    previous: index > 0 ? list[index - 1] : null,
    next: index < list.length - 1 ? list[index + 1] : null,
  };
}
