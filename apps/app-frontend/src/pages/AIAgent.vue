<script setup lang="ts">
import { EditIcon, PlusIcon, SparklesIcon, TrashIcon } from '@modrinth/assets'
import { Button, defineMessages, injectNotificationManager, useVIntl } from '@modrinth/ui'
import { fetch as tauriFetch } from '@tauri-apps/plugin-http'
import { computed, onMounted, onUnmounted, ref, shallowRef } from 'vue'
import { useRouter } from 'vue-router'

import { type McpToolDefinition, StreamableHttpMcpClient } from '@/assistant/mcp-client'
import {
    type McpServerConfig,
    readMcpServerConfigs,
    removeMcpServerConfig,
    upsertMcpServerConfig,
} from '@/assistant/mcp-server-config'
import LogAgentActivity from '@/components/ui/LogAgentActivity.vue'
import { useLogAgent } from '@/composables/useLogAgent'
import {
    type AIProviderConfig,
    type AIProviderDefinition,
    type AIProviderModel,
    type AIState,
    getAICatalog,
    getAIState,
} from '@/helpers/ai'
import {
    addAgentProfile,
    type AIAgentProfile,
    DEFAULT_AI_AGENT_PROFILE,
    getAgentProfiles,
    removeAgentProfile,
    updateAgentProfile,
} from '@/helpers/ai-agent-profiles'
import { list } from '@/helpers/instance'
import { get_crash_analysis_ai_settings } from '@/helpers/logs'
import type { GameInstance } from '@/helpers/types'
import { parseLogAgentInsight } from '@/logagent/result'
import {
    agentToolEnabled,
    type AssistantPermissionMode,
    BUILT_IN_AGENT_TOOLS,
    DANGEROUS_AGENT_TOOLS,
    enabledAgentTools,
    resetAgentTools,
    setAgentToolEnabled,
} from '@/logagent/tool-settings'
import type {
    AgentToolApproval,
    AnalysisSettings,
    RemoteMcpGateway,
    RemoteMcpTool,
} from '@/logagent/types'

const { formatMessage } = useVIntl()
const { handleError } = injectNotificationManager()
const router = useRouter()
const agent = useLogAgent()

const messages = defineMessages({
    title: { id: 'app.assistant.title', defaultMessage: 'AI assistant' },
    description: {
        id: 'app.assistant.description',
        defaultMessage: 'Get help with MC-Trin, Minecraft setup, and launcher problems.',
    },
    generalMode: { id: 'app.assistant.mode.general', defaultMessage: 'Launcher help' },
    logsMode: { id: 'app.assistant.mode.logs', defaultMessage: 'Log diagnosis' },
    generalPrivacy: {
        id: 'app.assistant.general-privacy',
        defaultMessage:
            'Your question, sanitized crash logs when log tools are enabled, and any requested launcher details are sent to your configured AI provider. The assistant only performs limited local writes after your approval.',
    },
    settings: { id: 'app.assistant.settings', defaultMessage: 'AI settings' },
    loading: {
        id: 'app.assistant.loading',
        defaultMessage: 'Loading AI and launcher status',
    },
    instance: { id: 'app.assistant.instance', defaultMessage: 'Instance' },
    question: { id: 'app.assistant.question', defaultMessage: 'Question' },
    questionPlaceholder: {
        id: 'app.assistant.question-placeholder',
        defaultMessage: 'What is the first error in this run?',
    },
    generalQuestionPlaceholder: {
        id: 'app.assistant.general-question-placeholder',
        defaultMessage: 'Describe the problem or ask a Minecraft question.',
    },
    ask: { id: 'app.assistant.ask', defaultMessage: 'Ask assistant' },
    analyze: { id: 'app.assistant.analyze', defaultMessage: 'Analyze logs' },
    stop: { id: 'app.assistant.stop', defaultMessage: 'Stop analysis' },
    result: { id: 'app.assistant.result', defaultMessage: 'Analysis' },
    rootCause: { id: 'app.assistant.result.root-cause', defaultMessage: 'Root cause' },
    evidence: { id: 'app.assistant.result.evidence', defaultMessage: 'Evidence' },
    steps: { id: 'app.assistant.result.steps', defaultMessage: 'Next steps' },
    createInstance: { id: 'app.assistant.create-instance', defaultMessage: 'Create an instance' },
    noInstances: {
        id: 'app.assistant.no-instances',
        defaultMessage: 'Create an instance before analyzing its logs.',
    },
    configureModel: {
        id: 'app.assistant.configure-model',
        defaultMessage: 'Enable AI and configure a provider with an active model first.',
    },
    configureAnalysis: {
        id: 'app.assistant.configure-analysis',
        defaultMessage: 'Enable local log analysis and choose the provider and model to use.',
    },
    configure: { id: 'app.assistant.configure', defaultMessage: 'Configure AI' },
    agentProfiles: { id: 'app.assistant.profiles.title', defaultMessage: 'Agents' },
    agentProfileLabel: { id: 'app.assistant.profiles.select', defaultMessage: 'Agent profile' },
    agentProfileHint: {
        id: 'app.assistant.profiles.hint',
        defaultMessage:
            'Profiles guide the assistant. Built-in tools are limited, and every write requires your approval.',
    },
    newAgentProfile: { id: 'app.assistant.profiles.create', defaultMessage: 'New agent' },
    editAgentProfile: { id: 'app.assistant.profiles.edit', defaultMessage: 'Edit agent' },
    deleteAgentProfile: { id: 'app.assistant.profiles.delete', defaultMessage: 'Delete agent' },
    agentProfileName: { id: 'app.assistant.profiles.name', defaultMessage: 'Agent name' },
    agentProfileInstructions: {
        id: 'app.assistant.profiles.instructions',
        defaultMessage: 'Agent instructions',
    },
    agentProfileSave: { id: 'app.assistant.profiles.save', defaultMessage: 'Save agent' },
    agentProfileCancel: { id: 'app.assistant.profiles.cancel', defaultMessage: 'Cancel' },
    agentProfileError: {
        id: 'app.assistant.profiles.error',
        defaultMessage: 'Check the agent name and instructions, then try again.',
    },
    agentToolsTitle: {
        id: 'app.assistant.tools.title',
        defaultMessage: 'Built-in tools',
    },
    agentToolsDescription: {
        id: 'app.assistant.tools.description',
        defaultMessage:
            'Choose which local tools the agent may call. Read tools expose sanitized data; write tools require approval for every call.',
    },
    agentToolsReset: {
        id: 'app.assistant.tools.reset',
        defaultMessage: 'Enable all',
    },
    agentToolsExternal: {
        id: 'app.assistant.tools.external',
        defaultMessage:
            'Need your own tools? Connect an MCP server below and its tools are offered to the assistant automatically.',
    },
    permissionModeTitle: {
        id: 'app.assistant.permission-mode.title',
        defaultMessage: 'Permission mode',
    },
    permissionModeDescription: {
        id: 'app.assistant.permission-mode.description',
        defaultMessage:
            'Decides when the assistant must ask before acting. The mode applies immediately; full access always resets when this page is reloaded.',
    },
    permissionModeDefault: {
        id: 'app.assistant.permission-mode.default',
        defaultMessage: 'Confirm every change',
    },
    permissionModeDefaultHint: {
        id: 'app.assistant.permission-mode.default-hint',
        defaultMessage: 'Every write and every MCP tool call asks for approval first.',
    },
    permissionModeFree: {
        id: 'app.assistant.permission-mode.free',
        defaultMessage: 'Free mode',
    },
    permissionModeFreeHint: {
        id: 'app.assistant.permission-mode.free-hint',
        defaultMessage:
            'Routine actions run without asking. Destructive actions (deleting instances, removing content files, changing launcher settings, installing runtimes) and every MCP tool still ask first.',
    },
    permissionModeFull: {
        id: 'app.assistant.permission-mode.full',
        defaultMessage: 'Full access',
    },
    permissionModeFullHint: {
        id: 'app.assistant.permission-mode.full-hint',
        defaultMessage:
            'The assistant runs every tool without asking, including destructive and MCP tools.',
    },
    permissionModeFullWarningTitle: {
        id: 'app.assistant.permission-mode.full-warning-title',
        defaultMessage: 'Enable full access?',
    },
    permissionModeFullWarningBody: {
        id: 'app.assistant.permission-mode.full-warning-body',
        defaultMessage:
            'The assistant will run every tool without asking: it can delete instances, remove content files, change launcher settings, install runtimes, and call any tool on the MCP servers you connected. Only continue if you trust the configured model and MCP servers.',
    },
    permissionModeFullWarningConfirm: {
        id: 'app.assistant.permission-mode.full-warning-confirm',
        defaultMessage: 'Enable full access',
    },
    permissionModeActiveBadge: {
        id: 'app.assistant.permission-mode.active-badge',
        defaultMessage: 'Full access is active: tools run without asking.',
    },
    usageGuideTitle: {
        id: 'app.assistant.usage-guide.title',
        defaultMessage: 'How to use the assistant',
    },
    usageStep1: {
        id: 'app.assistant.usage-guide.step-1',
        defaultMessage:
            'Open the AI settings to add an OpenAI-compatible provider with its endpoint and API key.',
    },
    usageStep2: {
        id: 'app.assistant.usage-guide.step-2',
        defaultMessage:
            'Pick an agent profile to give the assistant standing instructions, or keep the default.',
    },
    usageStep3: {
        id: 'app.assistant.usage-guide.step-3',
        defaultMessage:
            'Enable the built-in tools you need; connect an MCP server to offer your own tools.',
    },
    usageStep4: {
        id: 'app.assistant.usage-guide.step-4',
        defaultMessage:
            'Choose a permission mode. Start with confirm-every-change and relax it once you trust the setup.',
    },
    usageStep5: {
        id: 'app.assistant.usage-guide.step-5',
        defaultMessage:
            'Chat with the assistant. Reading launcher data is free; acting is gated by your permission mode.',
    },
    toolInstanceName: {
        id: 'app.assistant.tools.instance.name',
        defaultMessage: 'Instances',
    },
    toolInstanceDescription: {
        id: 'app.assistant.tools.instance.description',
        defaultMessage:
            'Names, stable IDs, game versions, loaders and install status. Paths stay hidden.',
    },
    toolSettingsName: {
        id: 'app.assistant.tools.settings.name',
        defaultMessage: 'Launcher settings',
    },
    toolSettingsDescription: {
        id: 'app.assistant.tools.settings.description',
        defaultMessage:
            'Language, memory and download preferences. Credentials and paths stay hidden.',
    },
    toolDiagnosticsName: {
        id: 'app.assistant.tools.diagnostics.name',
        defaultMessage: 'Platform diagnostics',
    },
    toolDiagnosticsDescription: {
        id: 'app.assistant.tools.diagnostics.description',
        defaultMessage: 'Operating system, architecture and memory totals for troubleshooting.',
    },
    toolJavaName: {
        id: 'app.assistant.tools.java.name',
        defaultMessage: 'Find installed Java',
    },
    toolJavaDescription: {
        id: 'app.assistant.tools.java.description',
        defaultMessage: 'Find local Java runtimes without exposing their paths.',
    },
    toolJavaVendorsName: {
        id: 'app.assistant.tools.java-vendors.name',
        defaultMessage: 'List Java vendors',
    },
    toolJavaVendorsDescription: {
        id: 'app.assistant.tools.java-vendors.description',
        defaultMessage: 'List Java vendors available for this platform.',
    },
    toolContentName: {
        id: 'app.assistant.tools.content.name',
        defaultMessage: 'Inspect instance content',
    },
    toolContentDescription: {
        id: 'app.assistant.tools.content.description',
        defaultMessage:
            'Inspect installed mods, resource packs, shaders, updates, and compatibility warnings.',
    },
    toolCheckUpdatesName: {
        id: 'app.assistant.tools.check-updates.name',
        defaultMessage: 'Check content updates',
    },
    toolCheckUpdatesDescription: {
        id: 'app.assistant.tools.check-updates.description',
        defaultMessage: 'Find compatible updates without installing them.',
    },
    toolSearchResourcesName: {
        id: 'app.assistant.tools.search-resources.name',
        defaultMessage: 'Search resources',
    },
    toolSearchResourcesDescription: {
        id: 'app.assistant.tools.search-resources.description',
        defaultMessage: 'Search configured Modrinth, CurseForge, and other resource sources.',
    },
    toolResourceReleasesName: {
        id: 'app.assistant.tools.resource-releases.name',
        defaultMessage: 'List resource releases',
    },
    toolResourceReleasesDescription: {
        id: 'app.assistant.tools.resource-releases.description',
        defaultMessage: 'List compatible versions for a resource project.',
    },
    toolPrepareInstallName: {
        id: 'app.assistant.tools.prepare-install.name',
        defaultMessage: 'Prepare resource install',
    },
    toolPrepareInstallDescription: {
        id: 'app.assistant.tools.prepare-install.description',
        defaultMessage: 'Resolve dependencies and compatibility before downloading anything.',
    },
    toolPrepareUpdateName: {
        id: 'app.assistant.tools.prepare-update.name',
        defaultMessage: 'Prepare content update',
    },
    toolPrepareUpdateDescription: {
        id: 'app.assistant.tools.prepare-update.description',
        defaultMessage: 'Prepare a dependency-aware update plan before changing installed files.',
    },
    toolInstallJobName: {
        id: 'app.assistant.tools.install-job.name',
        defaultMessage: 'Check installation job',
    },
    toolInstallJobDescription: {
        id: 'app.assistant.tools.install-job.description',
        defaultMessage: 'Read download and installation progress or errors.',
    },
    toolCancelJobName: {
        id: 'app.assistant.tools.cancel-job.name',
        defaultMessage: 'Cancel installation job',
    },
    toolCancelJobDescription: {
        id: 'app.assistant.tools.cancel-job.description',
        defaultMessage: 'Cancel a running Java or resource installation after approval.',
    },
    toolUpdateSettingsName: {
        id: 'app.assistant.tools.update-settings.name',
        defaultMessage: 'Change launcher settings',
    },
    toolUpdateSettingsDescription: {
        id: 'app.assistant.tools.update-settings.description',
        defaultMessage: 'Change language, theme, memory and download preferences after approval.',
    },
    toolCreateInstanceName: {
        id: 'app.assistant.tools.create-instance.name',
        defaultMessage: 'Create instance',
    },
    toolCreateInstanceDescription: {
        id: 'app.assistant.tools.create-instance.description',
        defaultMessage: 'Create an empty instance record; no files are downloaded.',
    },
    toolRenameInstanceName: {
        id: 'app.assistant.tools.rename-instance.name',
        defaultMessage: 'Rename instance',
    },
    toolRenameInstanceDescription: {
        id: 'app.assistant.tools.rename-instance.description',
        defaultMessage: 'Rename an instance by ID without exposing its path.',
    },
    toolDeleteInstanceName: {
        id: 'app.assistant.tools.delete-instance.name',
        defaultMessage: 'Delete instance',
    },
    toolDeleteInstanceDescription: {
        id: 'app.assistant.tools.delete-instance.description',
        defaultMessage: 'Delete an instance and managed files after approval.',
    },
    toolInstallJavaName: {
        id: 'app.assistant.tools.install-java.name',
        defaultMessage: 'Install Java',
    },
    toolInstallJavaDescription: {
        id: 'app.assistant.tools.install-java.description',
        defaultMessage: 'Download and install a compatible Java runtime after approval.',
    },
    toolSetJavaDefaultName: {
        id: 'app.assistant.tools.set-java-default.name',
        defaultMessage: 'Set default Java',
    },
    toolSetJavaDefaultDescription: {
        id: 'app.assistant.tools.set-java-default.description',
        defaultMessage: 'Set a discovered Java runtime as the default for its major version.',
    },
    toolQueueInstallName: {
        id: 'app.assistant.tools.queue-install.name',
        defaultMessage: 'Start resource installation',
    },
    toolQueueInstallDescription: {
        id: 'app.assistant.tools.queue-install.description',
        defaultMessage: 'Download and install a reviewed resource plan after approval.',
    },
    toolDisableContentName: {
        id: 'app.assistant.tools.disable-content.name',
        defaultMessage: 'Disable content',
    },
    toolDisableContentDescription: {
        id: 'app.assistant.tools.disable-content.description',
        defaultMessage: 'Temporarily disable a mod or resource to test a reversible fix.',
    },
    toolEnableContentName: {
        id: 'app.assistant.tools.enable-content.name',
        defaultMessage: 'Enable content',
    },
    toolEnableContentDescription: {
        id: 'app.assistant.tools.enable-content.description',
        defaultMessage: 'Restore a previously disabled content entry after approval.',
    },
    toolRemoveContentName: {
        id: 'app.assistant.tools.remove-content.name',
        defaultMessage: 'Remove content',
    },
    toolRemoveContentDescription: {
        id: 'app.assistant.tools.remove-content.description',
        defaultMessage: 'Remove one identified managed content file after approval.',
    },
    toolListLogsName: {
        id: 'app.assistant.tools.list-logs.name',
        defaultMessage: 'List log files',
    },
    toolListLogsDescription: {
        id: 'app.assistant.tools.list-logs.description',
        defaultMessage: 'The files captured in the current log snapshot.',
    },
    toolReadLogName: {
        id: 'app.assistant.tools.read-log.name',
        defaultMessage: 'Read log lines',
    },
    toolReadLogDescription: {
        id: 'app.assistant.tools.read-log.description',
        defaultMessage: 'Read up to 160 numbered lines from one snapshot file.',
    },
    toolGrepLogName: {
        id: 'app.assistant.tools.grep-log.name',
        defaultMessage: 'Search log lines',
    },
    toolGrepLogDescription: {
        id: 'app.assistant.tools.grep-log.description',
        defaultMessage: 'Find a literal string in one snapshot file with surrounding context.',
    },
    mcpTitle: { id: 'app.assistant.mcp.title', defaultMessage: 'MCP tools' },
    mcpEndpoint: { id: 'app.assistant.mcp.endpoint', defaultMessage: 'MCP server URL' },
    mcpEndpointPlaceholder: {
        id: 'app.assistant.mcp.endpoint-placeholder',
        defaultMessage: 'https://mcp.example.com/mcp',
    },
    mcpToken: { id: 'app.assistant.mcp.token', defaultMessage: 'Bearer token (kept in memory)' },
    mcpServerName: { id: 'app.assistant.mcp.name', defaultMessage: 'Server name' },
    mcpSavedServers: { id: 'app.assistant.mcp.saved-servers', defaultMessage: 'Saved MCP servers' },
    mcpSaveServer: { id: 'app.assistant.mcp.save-server', defaultMessage: 'Save server' },
    mcpUpdateServer: { id: 'app.assistant.mcp.update-server', defaultMessage: 'Update server' },
    mcpUseServer: { id: 'app.assistant.mcp.use-server', defaultMessage: 'Select server' },
    mcpRemoveServer: { id: 'app.assistant.mcp.remove-server', defaultMessage: 'Remove server' },
    mcpConfigError: {
        id: 'app.assistant.mcp.config-error',
        defaultMessage: 'Check the server name and HTTPS address, then try again.',
    },
    mcpConnect: { id: 'app.assistant.mcp.connect', defaultMessage: 'Connect' },
    mcpDisconnect: { id: 'app.assistant.mcp.disconnect', defaultMessage: 'Disconnect' },
    mcpDisconnected: { id: 'app.assistant.mcp.disconnected', defaultMessage: 'Not connected' },
    mcpConnected: {
        id: 'app.assistant.mcp.connected',
        defaultMessage: '{count, plural, one {# tool connected} other {# tools connected}}',
    },
    mcpApprovalTitle: {
        id: 'app.assistant.mcp.approval-title',
        defaultMessage: 'Review tool request',
    },
    localApprovalTitle: {
        id: 'app.assistant.tools.approval-title',
        defaultMessage: 'Review local write request',
    },
    mcpServer: { id: 'app.assistant.mcp.server', defaultMessage: 'Server' },
    mcpTool: { id: 'app.assistant.mcp.tool', defaultMessage: 'Tool' },
    mcpArguments: { id: 'app.assistant.mcp.arguments', defaultMessage: 'Arguments' },
    mcpApprove: { id: 'app.assistant.mcp.approve', defaultMessage: 'Allow this call' },
    mcpDeny: { id: 'app.assistant.mcp.deny', defaultMessage: 'Deny' },
})

const instances = ref<GameInstance[]>([])
const providerCatalog = ref<AIProviderDefinition[]>([])
const aiState = ref<AIState | null>(null)
const analysisSettings = ref<AnalysisSettings | null>(null)
const selectedInstanceId = ref('')
const mode = ref<'general' | 'logs'>('general')
const question = ref('')
const loading = ref(true)
const agentTools = BUILT_IN_AGENT_TOOLS
const toolEnabled = ref<Record<string, boolean>>({})
const toolResetNotice = ref(false)
const toolLabels = {
    get_instance_overview: {
        name: messages.toolInstanceName,
        description: messages.toolInstanceDescription,
    },
    get_launcher_settings: {
        name: messages.toolSettingsName,
        description: messages.toolSettingsDescription,
    },
    get_platform_diagnostics: {
        name: messages.toolDiagnosticsName,
        description: messages.toolDiagnosticsDescription,
    },
    discover_java_installations: {
        name: messages.toolJavaName,
        description: messages.toolJavaDescription,
    },
    list_java_vendors: {
        name: messages.toolJavaVendorsName,
        description: messages.toolJavaVendorsDescription,
    },
    inspect_instance_content: {
        name: messages.toolContentName,
        description: messages.toolContentDescription,
    },
    check_content_updates: {
        name: messages.toolCheckUpdatesName,
        description: messages.toolCheckUpdatesDescription,
    },
    search_resources: {
        name: messages.toolSearchResourcesName,
        description: messages.toolSearchResourcesDescription,
    },
    get_resource_releases: {
        name: messages.toolResourceReleasesName,
        description: messages.toolResourceReleasesDescription,
    },
    prepare_resource_install: {
        name: messages.toolPrepareInstallName,
        description: messages.toolPrepareInstallDescription,
    },
    prepare_resource_update: {
        name: messages.toolPrepareUpdateName,
        description: messages.toolPrepareUpdateDescription,
    },
    get_install_job: {
        name: messages.toolInstallJobName,
        description: messages.toolInstallJobDescription,
    },
    cancel_install_job: {
        name: messages.toolCancelJobName,
        description: messages.toolCancelJobDescription,
    },
    update_launcher_settings: {
        name: messages.toolUpdateSettingsName,
        description: messages.toolUpdateSettingsDescription,
    },
    create_instance: {
        name: messages.toolCreateInstanceName,
        description: messages.toolCreateInstanceDescription,
    },
    rename_instance: {
        name: messages.toolRenameInstanceName,
        description: messages.toolRenameInstanceDescription,
    },
    delete_instance: {
        name: messages.toolDeleteInstanceName,
        description: messages.toolDeleteInstanceDescription,
    },
    install_java_runtime: {
        name: messages.toolInstallJavaName,
        description: messages.toolInstallJavaDescription,
    },
    set_java_default_runtime: {
        name: messages.toolSetJavaDefaultName,
        description: messages.toolSetJavaDefaultDescription,
    },
    queue_resource_install: {
        name: messages.toolQueueInstallName,
        description: messages.toolQueueInstallDescription,
    },
    disable_content: {
        name: messages.toolDisableContentName,
        description: messages.toolDisableContentDescription,
    },
    enable_content: {
        name: messages.toolEnableContentName,
        description: messages.toolEnableContentDescription,
    },
    remove_content: {
        name: messages.toolRemoveContentName,
        description: messages.toolRemoveContentDescription,
    },
    list_log_files: {
        name: messages.toolListLogsName,
        description: messages.toolListLogsDescription,
    },
    read_log_file: { name: messages.toolReadLogName, description: messages.toolReadLogDescription },
    grep_log_file: { name: messages.toolGrepLogName, description: messages.toolGrepLogDescription },
} as const

function toolLabel(id: string) {
    return toolLabels[id as keyof typeof toolLabels] ?? toolLabels.get_instance_overview
}

function syncToolStates() {
    toolEnabled.value = Object.fromEntries(
        agentTools.map((tool) => [tool.id, agentToolEnabled(tool.id)]),
    )
}

function toggleAgentTool(id: string, enabled: boolean) {
    setAgentToolEnabled(id, enabled)
    syncToolStates()
    toolResetNotice.value = false
}

function enableAllAgentTools() {
    resetAgentTools()
    syncToolStates()
    toolResetNotice.value = true
}

const mcpEndpoint = ref('')
const mcpToken = ref('')
const mcpServerName = ref('')
const mcpServerConfigs = ref<McpServerConfig[]>([])
const selectedMcpServerId = ref('')
const mcpConfigError = ref('')
const mcpClient = shallowRef<StreamableHttpMcpClient | null>(null)
const remoteTools = ref<McpToolDefinition[]>([])
const mcpConnecting = ref(false)
const mcpError = ref('')
const agentProfiles = ref<AIAgentProfile[]>([DEFAULT_AI_AGENT_PROFILE])
const selectedAgentProfileId = ref(DEFAULT_AI_AGENT_PROFILE.id)
const profileDialogOpen = ref(false)
const editingProfileId = ref('')
const profileName = ref('')
const profileInstructions = ref('')
const profileError = ref('')

type ToolApproval = {
    endpoint: string
    tool: RemoteMcpTool
    args: Record<string, unknown>
    resolve: (approved: boolean) => void
}

const pendingApprovals = ref<ToolApproval[]>([])
const currentApproval = computed(() => pendingApprovals.value[0] ?? null)
const PERMISSION_MODE_STORAGE_KEY = 'mctrin.assistant.permission-mode.v1'
const permissionMode = ref<AssistantPermissionMode>('default')
const fullAccessWarningOpen = ref(false)

type AssistantModeMessage = { id: string; defaultMessage: string }
const permissionModes: {
    value: AssistantPermissionMode
    label: AssistantModeMessage
    hint: AssistantModeMessage
}[] = [
    {
        value: 'default',
        label: messages.permissionModeDefault,
        hint: messages.permissionModeDefaultHint,
    },
    {
        value: 'free',
        label: messages.permissionModeFree,
        hint: messages.permissionModeFreeHint,
    },
    {
        value: 'full',
        label: messages.permissionModeFull,
        hint: messages.permissionModeFullHint,
    },
]

function applyPermissionMode(mode: AssistantPermissionMode) {
    permissionMode.value = mode
    try {
        // Full access never survives a reload: it must be re-enabled with
        // the explicit warning once per session.
        if (mode === 'full') sessionStorage.removeItem(PERMISSION_MODE_STORAGE_KEY)
        else sessionStorage.setItem(PERMISSION_MODE_STORAGE_KEY, mode)
    } catch {
        // A blocked storage area keeps the choice for this session only.
    }
}

function setPermissionMode(mode: AssistantPermissionMode) {
    if (mode === permissionMode.value) return
    if (mode === 'full') {
        fullAccessWarningOpen.value = true
        return
    }
    applyPermissionMode(mode)
}

function confirmFullAccess() {
    fullAccessWarningOpen.value = false
    applyPermissionMode('full')
}
const selectedAgentProfile = computed(
    () =>
        agentProfiles.value.find((profile) => profile.id === selectedAgentProfileId.value) ??
        DEFAULT_AI_AGENT_PROFILE,
)
const enabledToolCount = computed(() => Object.values(toolEnabled.value).filter(Boolean).length)

const availableInstances = computed(() =>
    instances.value.filter((instance) => instance.install_stage === 'installed'),
)

const selectedLogModel = computed(() => {
    const settings = analysisSettings.value
    if (!aiState.value?.settings.enabled || !settings) return null
    const provider = aiState.value.providers.find(
        (candidate) => candidate.provider_id === settings.provider_id,
    )
    if (!provider?.enabled) return null
    const definition = providerCatalog.value.find((item) => item.id === provider.provider_id)
    const model = provider.models.find(
        (candidate) => candidate.id === settings.model_id && candidate.enabled,
    )
    if (!model) return null
    const hasCredential =
        definition?.auth_type === 'none' ||
        provider.has_api_key ||
        provider.oauth_connected ||
        provider.configured_credentials.length > 0
    return hasCredential ? { provider, model } : null
})

function firstUsableModel(
    provider: AIProviderConfig,
): { provider: AIProviderConfig; model: AIProviderModel } | null {
    if (!provider.enabled) return null
    const definition = providerCatalog.value.find((item) => item.id === provider.provider_id)
    const model = provider.models.find((candidate) => candidate.enabled)
    if (!model) return null
    const hasCredential =
        definition?.auth_type === 'none' ||
        provider.has_api_key ||
        provider.oauth_connected ||
        provider.configured_credentials.length > 0
    return hasCredential ? { provider, model } : null
}

const assistantModel = computed(() => {
    if (!aiState.value?.settings.enabled) return null
    if (selectedLogModel.value) return selectedLogModel.value
    for (const provider of aiState.value.providers) {
        const usable = firstUsableModel(provider)
        if (usable) return usable
    }
    return null
})

const hasConfiguredModel = computed(() => {
    if (mode.value === 'logs')
        return Boolean(selectedLogModel.value) && Boolean(analysisSettings.value?.enabled)
    return Boolean(assistantModel.value)
})

const settingsTarget = computed(() => {
    return mode.value === 'logs' && assistantModel.value ? '#logs' : '#ai'
})

const configurationMessage = computed(() => {
    return mode.value === 'logs' && assistantModel.value
        ? messages.configureAnalysis
        : messages.configureModel
})

const canSubmit = computed(
    () =>
        !loading.value &&
        !agent.busy.value &&
        hasConfiguredModel.value &&
        (mode.value === 'general' || Boolean(selectedInstanceId.value)) &&
        Boolean(question.value.trim()),
)
const parsedResult = computed(() => parseLogAgentInsight(agent.output.value))

async function load() {
    loading.value = true
    try {
        const [loadedInstances, catalog, state, localSettings] = await Promise.all([
            list(),
            getAICatalog(),
            getAIState(),
            get_crash_analysis_ai_settings(),
        ])
        instances.value = loadedInstances
        providerCatalog.value = catalog
        aiState.value = state
        analysisSettings.value = localSettings
        if (!availableInstances.value.some((instance) => instance.id === selectedInstanceId.value))
            selectedInstanceId.value = availableInstances.value[0]?.id ?? ''
    } catch (error) {
        handleError(error as Error)
    } finally {
        loading.value = false
    }
}

async function submit() {
    if (!canSubmit.value) return
    const prompt = question.value.trim()
    const enabledTools = enabledAgentTools(mode.value === 'general' ? 'assistant' : 'logs')
    const client = mcpClient.value
    const remoteMcp: RemoteMcpGateway | undefined = client
        ? {
              endpoint: client.endpoint,
              tools: remoteTools.value,
              approveTool: requestToolApproval,
              callTool: (name, args, signal) => client.callTool(name, args, signal),
          }
        : undefined
    if (mode.value === 'logs') {
        await agent.start(
            selectedInstanceId.value,
            'custom',
            null,
            () => {},
            prompt,
            remoteMcp,
            undefined,
            '',
            undefined,
            enabledTools.has('read_log_file') || enabledTools.has('grep_log_file'),
        )
        return
    }
    const model = assistantModel.value
    if (!model) return
    await agent.start(
        selectedInstanceId.value,
        'assistant',
        null,
        () => {},
        prompt,
        remoteMcp,
        {
            providerId: model.provider.provider_id,
            modelId: model.model.id,
        },
        selectedAgentProfile.value.instructions,
        requestLocalToolApproval,
        enabledTools.has('read_log_file') || enabledTools.has('grep_log_file'),
    )
}

function openAISettings() {
    void router.push({ path: '/settings', hash: '#ai' })
}

function openRequiredSettings() {
    void router.push({ path: '/settings', hash: settingsTarget.value })
}

function selectMode(next: 'general' | 'logs') {
    if (mode.value === next || agent.busy.value) return
    mode.value = next
    question.value = ''
    agent.reset()
}

async function connectMcp() {
    mcpConnecting.value = true
    mcpError.value = ''
    mcpClient.value = null
    remoteTools.value = []
    try {
        const client = new StreamableHttpMcpClient(mcpEndpoint.value, mcpToken.value, (url, init) =>
            tauriFetch(url, init),
        )
        const tools = await client.connect()
        mcpClient.value = client
        remoteTools.value = [...tools]
        mcpEndpoint.value = client.endpoint
        try {
            localStorage.setItem('mctrin.assistant.mcp-endpoint.v1', client.endpoint)
        } catch {
            // The endpoint remains available for this window if storage is disabled.
        }
        mcpToken.value = ''
    } catch (error) {
        const message = error instanceof Error ? error.message : String(error)
        mcpError.value = mcpToken.value ? message.replaceAll(mcpToken.value, '[redacted]') : message
    } finally {
        mcpConnecting.value = false
    }
}

function disconnectMcp() {
    denyPendingApprovals()
    mcpClient.value = null
    remoteTools.value = []
    mcpError.value = ''
}

function selectMcpServer(server: McpServerConfig) {
    if (agent.busy.value) return
    disconnectMcp()
    selectedMcpServerId.value = server.id
    mcpServerName.value = server.name
    mcpEndpoint.value = server.endpoint
    mcpConfigError.value = ''
}

function saveMcpServer() {
    try {
        const id = selectedMcpServerId.value || crypto.randomUUID()
        const next = upsertMcpServerConfig(localStorage, {
            id,
            name: mcpServerName.value,
            endpoint: mcpEndpoint.value,
        })
        mcpServerConfigs.value = next
        selectedMcpServerId.value = id
        mcpEndpoint.value = next.find((server) => server.id === id)?.endpoint ?? mcpEndpoint.value
        mcpConfigError.value = ''
    } catch {
        mcpConfigError.value = formatMessage(messages.mcpConfigError)
    }
}

function removeMcpServer(server: McpServerConfig) {
    try {
        mcpServerConfigs.value = removeMcpServerConfig(localStorage, server.id)
        if (selectedMcpServerId.value === server.id) {
            disconnectMcp()
            selectedMcpServerId.value = ''
            mcpServerName.value = ''
            mcpEndpoint.value = ''
        }
    } catch {
        mcpConfigError.value = formatMessage(messages.mcpConfigError)
    }
}

function beginNewAgentProfile() {
    editingProfileId.value = ''
    profileName.value = ''
    profileInstructions.value = ''
    profileError.value = ''
    profileDialogOpen.value = true
}

function beginEditAgentProfile() {
    const profile = selectedAgentProfile.value
    if (profile.id === DEFAULT_AI_AGENT_PROFILE.id) return
    editingProfileId.value = profile.id
    profileName.value = profile.name
    profileInstructions.value = profile.instructions
    profileError.value = ''
    profileDialogOpen.value = true
}

function saveAgentProfile() {
    try {
        agentProfiles.value = editingProfileId.value
            ? updateAgentProfile(localStorage, editingProfileId.value, {
                  name: profileName.value,
                  instructions: profileInstructions.value,
              })
            : addAgentProfile(localStorage, {
                  name: profileName.value,
                  instructions: profileInstructions.value,
              })
        const saved = editingProfileId.value
            ? agentProfiles.value.find((profile) => profile.id === editingProfileId.value)
            : agentProfiles.value.at(-1)
        if (saved) selectedAgentProfileId.value = saved.id
        profileDialogOpen.value = false
    } catch {
        profileError.value = formatMessage(messages.agentProfileError)
    }
}

function deleteAgentProfile() {
    const profile = selectedAgentProfile.value
    if (profile.id === DEFAULT_AI_AGENT_PROFILE.id) return
    try {
        agentProfiles.value = removeAgentProfile(localStorage, profile.id)
        selectedAgentProfileId.value = DEFAULT_AI_AGENT_PROFILE.id
    } catch {
        profileError.value = formatMessage(messages.agentProfileError)
    }
}

function requestToolApproval(
    tool: RemoteMcpTool,
    args: Record<string, unknown>,
    endpointOverride?: string,
) {
    const endpoint = endpointOverride ?? mcpClient.value?.endpoint
    if (!endpoint) return Promise.resolve(false)
    if (permissionMode.value === 'full') return Promise.resolve(true)
    if (permissionMode.value === 'free' && !isDangerousTool(tool.name, endpoint === 'MC-Trin')) {
        return Promise.resolve(true)
    }
    return new Promise<boolean>((resolve) => {
        pendingApprovals.value = [...pendingApprovals.value, { endpoint, tool, args, resolve }]
    })
}

function isDangerousTool(toolName: string, local: boolean) {
    // Remote MCP tools run on servers this launcher does not control, so
    // every mode below full access keeps asking before calling them.
    return !local || DANGEROUS_AGENT_TOOLS.has(toolName)
}

function requestLocalToolApproval(tool: AgentToolApproval, args: Record<string, unknown>) {
    return requestToolApproval({ name: tool.name, description: tool.description }, args, 'MC-Trin')
}

function resolveToolApproval(approved: boolean) {
    const [request, ...remaining] = pendingApprovals.value
    pendingApprovals.value = remaining
    request?.resolve(approved)
}

function denyPendingApprovals() {
    const requests = pendingApprovals.value
    pendingApprovals.value = []
    requests.forEach((request) => request.resolve(false))
}

function cancelAgent() {
    denyPendingApprovals()
    agent.cancel()
}

onMounted(() => {
    syncToolStates()
    // Only the free mode is restored; full access must be re-enabled with
    // its warning once per session.
    try {
        permissionMode.value =
            sessionStorage.getItem(PERMISSION_MODE_STORAGE_KEY) === 'free' ? 'free' : 'default'
    } catch {
        permissionMode.value = 'default'
    }
    try {
        agentProfiles.value = getAgentProfiles(localStorage)
        selectedAgentProfileId.value = agentProfiles.value[0]?.id ?? DEFAULT_AI_AGENT_PROFILE.id
        mcpServerConfigs.value = readMcpServerConfigs(localStorage)
        mcpEndpoint.value = localStorage.getItem('mctrin.assistant.mcp-endpoint.v1') ?? ''
        const saved = mcpServerConfigs.value.find((server) => server.endpoint === mcpEndpoint.value)
        if (saved) {
            selectedMcpServerId.value = saved.id
            mcpServerName.value = saved.name
        }
    } catch {
        mcpEndpoint.value = ''
        mcpServerConfigs.value = []
        agentProfiles.value = [DEFAULT_AI_AGENT_PROFILE]
    }
    void load()
})
onUnmounted(denyPendingApprovals)
</script>

<template>
    <main class="box-border flex min-h-full w-full flex-col gap-4 p-6">
        <header class="flex flex-wrap items-start justify-between gap-3">
            <div>
                <h1 class="m-0 text-2xl font-semibold text-[var(--color-text-primary)]">
                    <SparklesIcon class="mr-2 inline size-5 align-[-3px] text-brand" />
                    {{ formatMessage(messages.title) }}
                </h1>
                <p class="mb-0 mt-1 text-sm text-[var(--color-text-tertiary)]">
                    {{ formatMessage(messages.description) }}
                </p>
            </div>
            <Button type="outlined" @click="openAISettings">{{
                formatMessage(messages.settings)
            }}</Button>
        </header>

        <p v-if="loading" class="m-0 text-sm text-[var(--color-text-tertiary)]" role="status">
            {{ formatMessage(messages.loading) }}
        </p>

        <template v-else>
            <div
                class="flex flex-wrap gap-2"
                role="tablist"
                :aria-label="formatMessage(messages.title)"
            >
                <Button
                    role="tab"
                    :aria-selected="mode === 'general'"
                    :disabled="agent.busy.value"
                    :type="mode === 'general' ? 'colored' : 'outlined'"
                    color="brand"
                    @click="selectMode('general')"
                >
                    {{ formatMessage(messages.generalMode) }}
                </Button>
                <Button
                    role="tab"
                    :aria-selected="mode === 'logs'"
                    :disabled="agent.busy.value"
                    :type="mode === 'logs' ? 'colored' : 'outlined'"
                    color="brand"
                    @click="selectMode('logs')"
                >
                    {{ formatMessage(messages.logsMode) }}
                </Button>
            </div>

            <section
                v-if="mode === 'general'"
                class="flex max-w-4xl flex-col gap-3 border-b border-solid border-surface-4 pb-4"
                aria-labelledby="assistant-agent-profiles-title"
            >
                <h2
                    id="assistant-agent-profiles-title"
                    class="m-0 text-base font-semibold text-[var(--color-text-primary)]"
                >
                    {{ formatMessage(messages.agentProfiles) }}
                </h2>
                <div class="flex min-w-0 flex-wrap items-center gap-2">
                    <label class="flex min-w-48 flex-1 flex-col gap-1.5 text-sm font-semibold">
                        <span>{{ formatMessage(messages.agentProfileLabel) }}</span>
                        <select
                            v-model="selectedAgentProfileId"
                            class="h-10 min-w-0 rounded-md border border-solid border-surface-5 bg-surface-2 px-3 text-sm font-normal text-[var(--color-text-primary)]"
                            :disabled="agent.busy.value"
                        >
                            <option
                                v-for="profile in agentProfiles"
                                :key="profile.id"
                                :value="profile.id"
                            >
                                {{ profile.name }}
                            </option>
                        </select>
                    </label>
                    <Button
                        type="outlined"
                        class="mt-5"
                        :disabled="agent.busy.value"
                        @click="beginNewAgentProfile"
                    >
                        <PlusIcon />
                        {{ formatMessage(messages.newAgentProfile) }}
                    </Button>
                    <Button
                        v-if="selectedAgentProfile.id !== DEFAULT_AI_AGENT_PROFILE.id"
                        type="outlined"
                        class="mt-5"
                        :disabled="agent.busy.value"
                        @click="beginEditAgentProfile"
                    >
                        <EditIcon />
                        {{ formatMessage(messages.editAgentProfile) }}
                    </Button>
                    <Button
                        v-if="selectedAgentProfile.id !== DEFAULT_AI_AGENT_PROFILE.id"
                        type="quiet"
                        class="mt-5"
                        :disabled="agent.busy.value"
                        :aria-label="formatMessage(messages.deleteAgentProfile)"
                        @click="deleteAgentProfile"
                    >
                        <TrashIcon />
                    </Button>
                </div>
                <p class="m-0 max-w-3xl text-xs text-[var(--color-text-tertiary)]">
                    {{ formatMessage(messages.agentProfileHint) }}
                </p>
            </section>

            <p
                v-if="!hasConfiguredModel"
                class="m-0 flex flex-wrap items-center gap-2 rounded-md border border-solid border-surface-4 bg-surface-2 p-3 text-sm text-[var(--color-text-tertiary)]"
                role="status"
            >
                <span>{{ formatMessage(configurationMessage) }}</span>
                <Button type="outlined" @click="openRequiredSettings">{{
                    formatMessage(messages.configure)
                }}</Button>
            </p>

            <p
                v-if="mode === 'logs' && !availableInstances.length"
                class="m-0 flex flex-wrap items-center gap-3 text-sm text-[var(--color-text-tertiary)]"
            >
                <span>{{ formatMessage(messages.noInstances) }}</span>
                <Button type="outlined" @click="router.push('/create')">
                    {{ formatMessage(messages.createInstance) }}
                </Button>
            </p>

            <p
                v-if="mode === 'general'"
                class="m-0 max-w-4xl text-sm text-[var(--color-text-tertiary)]"
            >
                {{ formatMessage(messages.generalPrivacy) }}
            </p>

            <details
                class="assistant-usage max-w-4xl rounded-md border border-solid border-surface-4 bg-surface-2 p-3"
            >
                <summary
                    class="cursor-pointer text-base font-semibold text-[var(--color-text-primary)]"
                >
                    {{ formatMessage(messages.usageGuideTitle) }}
                </summary>
                <ol
                    class="mt-3 flex list-decimal flex-col gap-1.5 pl-5 text-sm text-[var(--color-text-tertiary)]"
                >
                    <li>{{ formatMessage(messages.usageStep1) }}</li>
                    <li>{{ formatMessage(messages.usageStep2) }}</li>
                    <li>{{ formatMessage(messages.usageStep3) }}</li>
                    <li>{{ formatMessage(messages.usageStep4) }}</li>
                    <li>{{ formatMessage(messages.usageStep5) }}</li>
                </ol>
            </details>

            <details
                class="assistant-tools order-4 max-w-4xl rounded-md border border-solid border-surface-4 bg-surface-2 p-3"
            >
                <summary
                    class="cursor-pointer text-base font-semibold text-[var(--color-text-primary)]"
                >
                    {{ formatMessage(messages.agentToolsTitle) }}
                    <span class="ml-2 text-xs font-normal text-[var(--color-text-tertiary)]">
                        {{ enabledToolCount }}/{{ agentTools.length }}
                    </span>
                </summary>
                <div class="mt-3 flex flex-col gap-3">
                    <div class="flex flex-wrap items-center justify-between gap-2">
                        <p class="m-0 text-sm text-[var(--color-text-tertiary)]">
                            {{ formatMessage(messages.agentToolsDescription) }}
                        </p>
                        <Button
                            type="outlined"
                            :disabled="agent.busy.value"
                            @click="enableAllAgentTools"
                        >
                            {{ formatMessage(messages.agentToolsReset) }}
                        </Button>
                    </div>
                    <p v-if="toolResetNotice" class="m-0 text-sm text-brand" role="status">
                        {{ formatMessage(messages.agentToolsReset) }}
                    </p>
                    <fieldset
                        class="assistant-permissions m-0 flex flex-col gap-2 rounded-md border border-solid border-surface-4 p-3"
                    >
                        <legend class="px-1 text-sm font-semibold text-[var(--color-text-primary)]">
                            {{ formatMessage(messages.permissionModeTitle) }}
                        </legend>
                        <p class="m-0 text-xs text-[var(--color-text-tertiary)]">
                            {{ formatMessage(messages.permissionModeDescription) }}
                        </p>
                        <div class="flex flex-wrap gap-2">
                            <label
                                v-for="option in permissionModes"
                                :key="option.value"
                                class="flex min-w-0 cursor-pointer items-start gap-2 rounded-md border border-solid p-2 text-sm"
                                :class="
                                    permissionMode === option.value
                                        ? 'border-brand bg-surface-3'
                                        : 'border-surface-4'
                                "
                            >
                                <input
                                    type="radio"
                                    name="assistant-permission-mode"
                                    class="mt-0.5 h-4 w-4"
                                    :checked="permissionMode === option.value"
                                    :disabled="agent.busy.value"
                                    @change="setPermissionMode(option.value)"
                                />
                                <span class="min-w-0">
                                    <span
                                        class="block font-semibold text-[var(--color-text-primary)]"
                                        >{{ formatMessage(option.label) }}</span
                                    >
                                    <span
                                        class="block text-xs font-normal text-[var(--color-text-tertiary)]"
                                        >{{ formatMessage(option.hint) }}</span
                                    >
                                </span>
                            </label>
                        </div>
                        <p
                            v-if="permissionMode === 'full'"
                            class="m-0 flex flex-wrap items-center gap-2 text-sm font-semibold text-red"
                            role="status"
                        >
                            {{ formatMessage(messages.permissionModeActiveBadge) }}
                            <button
                                type="button"
                                class="cursor-pointer text-xs font-normal underline"
                                @click="applyPermissionMode('default')"
                            >
                                {{ formatMessage(messages.permissionModeDefault) }}
                            </button>
                        </p>
                        <p
                            v-if="permissionMode === 'free'"
                            class="m-0 rounded-md border border-solid border-brand bg-surface-2 p-3 text-sm text-[var(--color-text-primary)]"
                            role="status"
                        >
                            {{ formatMessage(messages.permissionModeFreeHint) }}
                        </p>
                    </fieldset>
                    <ul class="m-0 flex flex-col gap-2 p-0">
                        <li
                            v-for="tool in agentTools"
                            :key="tool.id"
                            class="flex min-w-0 items-start justify-between gap-3 border-t border-solid border-surface-4 pt-2"
                        >
                            <div class="min-w-0 flex-1">
                                <strong
                                    class="block break-words text-sm text-[var(--color-text-primary)]"
                                >
                                    {{ formatMessage(toolLabel(tool.id).name) }}
                                </strong>
                                <span class="block text-xs text-[var(--color-text-tertiary)]">
                                    {{ formatMessage(toolLabel(tool.id).description) }}
                                </span>
                                <span
                                    v-if="tool.permission === 'write'"
                                    class="block text-xs font-semibold text-brand"
                                >
                                    {{ formatMessage(messages.localApprovalTitle) }}
                                </span>
                            </div>
                            <label class="flex shrink-0 items-center gap-2 text-sm">
                                <input
                                    type="checkbox"
                                    class="h-4 w-4"
                                    :checked="toolEnabled[tool.id]"
                                    :aria-label="formatMessage(toolLabel(tool.id).name)"
                                    @change="
                                        toggleAgentTool(
                                            tool.id,
                                            ($event.target as HTMLInputElement).checked,
                                        )
                                    "
                                />
                            </label>
                        </li>
                    </ul>
                    <p class="m-0 text-sm text-[var(--color-text-tertiary)]">
                        {{ formatMessage(messages.agentToolsExternal) }}
                    </p>
                </div>
            </details>

            <details
                class="assistant-mcp order-6 max-w-4xl rounded-md border border-solid border-surface-4 bg-surface-2 p-3"
            >
                <summary
                    class="cursor-pointer text-base font-semibold text-[var(--color-text-primary)]"
                >
                    {{ formatMessage(messages.mcpTitle) }}
                    <span class="ml-2 text-xs font-normal text-[var(--color-text-tertiary)]">
                        {{
                            mcpClient
                                ? formatMessage(messages.mcpConnected, {
                                      count: remoteTools.length,
                                  })
                                : formatMessage(messages.mcpDisconnected)
                        }}
                    </span>
                </summary>
                <div class="mt-3 flex flex-col gap-3">
                    <div class="grid grid-cols-1 gap-3 md:grid-cols-3">
                        <label class="flex min-w-0 flex-col gap-1.5 text-sm font-semibold">
                            <span>{{ formatMessage(messages.mcpServerName) }}</span>
                            <input
                                v-model="mcpServerName"
                                class="h-10 min-w-0 rounded-md border border-solid border-surface-5 bg-surface-2 px-3 text-sm font-normal text-[var(--color-text-primary)]"
                                type="text"
                                maxlength="64"
                                autocomplete="off"
                            />
                        </label>
                        <label class="flex min-w-0 flex-col gap-1.5 text-sm font-semibold">
                            <span>{{ formatMessage(messages.mcpEndpoint) }}</span>
                            <input
                                v-model="mcpEndpoint"
                                class="h-10 min-w-0 rounded-md border border-solid border-surface-5 bg-surface-2 px-3 text-sm font-normal text-[var(--color-text-primary)]"
                                type="url"
                                autocomplete="url"
                                :placeholder="formatMessage(messages.mcpEndpointPlaceholder)"
                            />
                        </label>
                        <label class="flex min-w-0 flex-col gap-1.5 text-sm font-semibold">
                            <span>{{ formatMessage(messages.mcpToken) }}</span>
                            <input
                                v-model="mcpToken"
                                class="h-10 min-w-0 rounded-md border border-solid border-surface-5 bg-surface-2 px-3 text-sm font-normal text-[var(--color-text-primary)]"
                                type="password"
                                autocomplete="off"
                            />
                        </label>
                    </div>
                    <div class="flex flex-wrap items-center gap-2">
                        <Button
                            type="outlined"
                            :disabled="
                                agent.busy.value || !mcpServerName.trim() || !mcpEndpoint.trim()
                            "
                            @click="saveMcpServer"
                        >
                            {{
                                selectedMcpServerId
                                    ? formatMessage(messages.mcpUpdateServer)
                                    : formatMessage(messages.mcpSaveServer)
                            }}
                        </Button>
                        <Button
                            type="outlined"
                            :disabled="mcpConnecting || agent.busy.value || !mcpEndpoint.trim()"
                            @click="connectMcp"
                        >
                            {{ formatMessage(messages.mcpConnect) }}
                        </Button>
                        <Button
                            v-if="mcpClient"
                            type="outlined"
                            :disabled="agent.busy.value"
                            @click="disconnectMcp"
                        >
                            {{ formatMessage(messages.mcpDisconnect) }}
                        </Button>
                        <span
                            v-if="mcpClient"
                            class="text-sm text-[var(--color-text-tertiary)]"
                            role="status"
                        >
                            {{
                                formatMessage(messages.mcpConnected, { count: remoteTools.length })
                            }}
                        </span>
                        <span
                            v-else-if="!mcpError"
                            class="text-sm text-[var(--color-text-tertiary)]"
                            role="status"
                        >
                            {{ formatMessage(messages.mcpDisconnected) }}
                        </span>
                        <span v-if="mcpError" class="text-sm text-red" role="alert">{{
                            mcpError
                        }}</span>
                        <span v-if="mcpConfigError" class="text-sm text-red" role="alert">{{
                            mcpConfigError
                        }}</span>
                    </div>
                    <ul
                        v-if="mcpServerConfigs.length"
                        class="m-0 flex flex-col gap-2 p-0"
                        :aria-label="formatMessage(messages.mcpSavedServers)"
                    >
                        <li
                            v-for="server in mcpServerConfigs"
                            :key="server.id"
                            class="flex min-w-0 flex-wrap items-center justify-between gap-2 border-t border-solid border-surface-4 pt-2"
                        >
                            <div class="min-w-0 flex-1">
                                <strong
                                    class="block break-words text-sm text-[var(--color-text-primary)]"
                                >
                                    {{ server.name }}
                                </strong>
                                <span
                                    class="block break-all text-xs text-[var(--color-text-tertiary)]"
                                >
                                    {{ server.endpoint }}
                                </span>
                            </div>
                            <div class="flex shrink-0 items-center gap-1">
                                <Button
                                    type="outlined"
                                    :disabled="agent.busy.value"
                                    @click="selectMcpServer(server)"
                                >
                                    {{ formatMessage(messages.mcpUseServer) }}
                                </Button>
                                <Button
                                    type="quiet"
                                    :disabled="agent.busy.value"
                                    :aria-label="formatMessage(messages.mcpRemoveServer)"
                                    @click="removeMcpServer(server)"
                                >
                                    <TrashIcon />
                                </Button>
                            </div>
                        </li>
                    </ul>
                    <ul v-if="remoteTools.length" class="m-0 flex flex-col gap-2 p-0 text-sm">
                        <li
                            v-for="tool in remoteTools"
                            :key="tool.name"
                            class="flex min-w-0 flex-wrap items-baseline gap-x-2 border-t border-solid border-surface-4 pt-2"
                        >
                            <strong class="break-all text-[var(--color-text-primary)]">{{
                                tool.name
                            }}</strong>
                            <span
                                v-if="tool.description"
                                class="min-w-0 break-words text-[var(--color-text-tertiary)]"
                            >
                                {{ tool.description }}
                            </span>
                        </li>
                    </ul>
                </div>
            </details>

            <form
                v-if="mode === 'general' || availableInstances.length"
                class="order-2 flex max-w-4xl flex-col gap-4"
                @submit.prevent="submit"
            >
                <label
                    v-if="mode === 'logs' || availableInstances.length"
                    class="flex max-w-2xl flex-col gap-1.5 text-sm font-semibold"
                >
                    <span>{{ formatMessage(messages.instance) }}</span>
                    <select
                        v-model="selectedInstanceId"
                        class="h-10 rounded-md border border-solid border-surface-5 bg-surface-2 px-3 text-[var(--color-text-primary)]"
                        :required="mode === 'logs'"
                    >
                        <option
                            v-for="instance in availableInstances"
                            :key="instance.id"
                            :value="instance.id"
                        >
                            {{ instance.name }} · {{ instance.game_version }}
                        </option>
                    </select>
                </label>

                <label class="flex flex-col gap-1.5 text-sm font-semibold">
                    <span>{{ formatMessage(messages.question) }}</span>
                    <textarea
                        v-model="question"
                        class="min-h-24 resize-y rounded-md border border-solid border-surface-5 bg-surface-2 p-3 text-sm font-normal text-[var(--color-text-primary)]"
                        maxlength="2000"
                        required
                        rows="3"
                        :placeholder="
                            formatMessage(
                                mode === 'general'
                                    ? messages.generalQuestionPlaceholder
                                    : messages.questionPlaceholder,
                            )
                        "
                    />
                </label>

                <div class="flex flex-wrap gap-2">
                    <Button
                        type="colored"
                        color="brand"
                        :disabled="!canSubmit"
                        @click.prevent="submit"
                    >
                        {{ formatMessage(mode === 'general' ? messages.ask : messages.analyze) }}
                    </Button>
                    <Button v-if="agent.busy.value" type="outlined" @click="cancelAgent">
                        {{ formatMessage(messages.stop) }}
                    </Button>
                </div>
            </form>

            <LogAgentActivity
                class="order-3"
                :entries="agent.activity.value"
                :busy="agent.busy.value"
                :elapsed="agent.elapsed.value"
            />

            <section
                v-if="agent.error.value"
                class="assistant-result order-3 max-w-4xl"
                role="alert"
            >
                <h2 class="mb-2 mt-0 text-base font-semibold text-red">
                    {{ formatMessage(messages.result) }}
                </h2>
                <p class="m-0 whitespace-pre-wrap break-words text-sm text-red">
                    {{ agent.error.value }}
                </p>
            </section>

            <section v-if="agent.output.value" class="assistant-result order-3 max-w-4xl">
                <h2 class="mb-2 mt-0 text-base font-semibold text-[var(--color-text-primary)]">
                    {{ formatMessage(messages.result) }}
                </h2>
                <pre
                    class="m-0 overflow-x-auto whitespace-pre-wrap break-words rounded-md border border-solid border-surface-4 bg-surface-2 p-4 text-sm text-[var(--color-text-primary)]"
                    >{{ parsedResult.markdown }}</pre>
                <div
                    v-if="parsedResult.insight"
                    class="mt-3 flex flex-col gap-3 rounded-md border border-solid border-surface-4 bg-surface-2 p-4 text-sm"
                >
                    <div>
                        <h3 class="m-0 text-sm font-semibold text-[var(--color-text-primary)]">
                            {{ formatMessage(messages.rootCause) }}
                        </h3>
                        <p class="m-0 mt-1 whitespace-pre-wrap break-words">
                            {{ parsedResult.insight.rootCause }}
                        </p>
                    </div>
                    <div v-if="parsedResult.insight.evidence.length">
                        <h3 class="m-0 text-sm font-semibold text-[var(--color-text-primary)]">
                            {{ formatMessage(messages.evidence) }}
                        </h3>
                        <ul class="m-0 mt-1 list-disc pl-5">
                            <li v-for="item in parsedResult.insight.evidence" :key="item">
                                {{ item }}
                            </li>
                        </ul>
                    </div>
                    <div v-if="parsedResult.insight.steps.length">
                        <h3 class="m-0 text-sm font-semibold text-[var(--color-text-primary)]">
                            {{ formatMessage(messages.steps) }}
                        </h3>
                        <ol class="m-0 mt-1 list-decimal pl-5">
                            <li v-for="item in parsedResult.insight.steps" :key="item">
                                {{ item }}
                            </li>
                        </ol>
                    </div>
                </div>
            </section>
        </template>

        <div
            v-if="fullAccessWarningOpen"
            class="fixed inset-0 z-[500] flex items-center justify-center bg-black/60 p-4"
            role="presentation"
            @click.self="fullAccessWarningOpen = false"
        >
            <section
                class="flex max-h-[85vh] w-full max-w-2xl flex-col gap-3 overflow-y-auto rounded-lg border border-solid border-surface-5 bg-surface-2 p-4 shadow-xl"
                role="alertdialog"
                aria-modal="true"
                aria-labelledby="assistant-full-access-title"
            >
                <h2 id="assistant-full-access-title" class="m-0 text-lg font-semibold text-red">
                    {{ formatMessage(messages.permissionModeFullWarningTitle) }}
                </h2>
                <p class="m-0 break-words text-sm text-[var(--color-text-primary)]">
                    {{ formatMessage(messages.permissionModeFullWarningBody) }}
                </p>
                <div class="flex flex-wrap justify-end gap-2">
                    <Button type="outlined" @click="fullAccessWarningOpen = false">
                        {{ formatMessage(messages.agentProfileCancel) }}
                    </Button>
                    <Button type="colored" color="danger" @click="confirmFullAccess">
                        {{ formatMessage(messages.permissionModeFullWarningConfirm) }}
                    </Button>
                </div>
            </section>
        </div>

        <div
            v-if="profileDialogOpen"
            class="fixed inset-0 z-[500] flex items-center justify-center bg-black/60 p-4"
            role="presentation"
            @click.self="profileDialogOpen = false"
        >
            <form
                class="flex max-h-[85vh] w-full max-w-2xl flex-col gap-4 overflow-y-auto rounded-md border border-solid border-surface-5 bg-surface-2 p-5 shadow-xl"
                role="dialog"
                aria-modal="true"
                aria-labelledby="assistant-agent-profile-editor-title"
                @submit.prevent="saveAgentProfile"
            >
                <h2
                    id="assistant-agent-profile-editor-title"
                    class="m-0 text-lg font-semibold text-[var(--color-text-primary)]"
                >
                    {{
                        formatMessage(
                            editingProfileId ? messages.editAgentProfile : messages.newAgentProfile,
                        )
                    }}
                </h2>
                <label class="flex min-w-0 flex-col gap-1.5 text-sm font-semibold">
                    <span>{{ formatMessage(messages.agentProfileName) }}</span>
                    <input
                        v-model="profileName"
                        class="h-10 min-w-0 rounded-md border border-solid border-surface-5 bg-surface-1 px-3 text-sm font-normal text-[var(--color-text-primary)]"
                        maxlength="40"
                        required
                        autofocus
                    />
                </label>
                <label class="flex min-w-0 flex-col gap-1.5 text-sm font-semibold">
                    <span>{{ formatMessage(messages.agentProfileInstructions) }}</span>
                    <textarea
                        v-model="profileInstructions"
                        class="min-h-36 min-w-0 resize-y rounded-md border border-solid border-surface-5 bg-surface-1 p-3 text-sm font-normal text-[var(--color-text-primary)]"
                        maxlength="2000"
                        rows="6"
                    />
                </label>
                <p v-if="profileError" class="m-0 text-sm text-red" role="alert">
                    {{ profileError }}
                </p>
                <div class="flex flex-wrap justify-end gap-2">
                    <Button type="outlined" @click="profileDialogOpen = false">
                        {{ formatMessage(messages.agentProfileCancel) }}
                    </Button>
                    <Button type="colored" color="brand" native-type="submit">
                        <PlusIcon />
                        {{ formatMessage(messages.agentProfileSave) }}
                    </Button>
                </div>
            </form>
        </div>

        <div
            v-if="currentApproval"
            class="fixed inset-0 z-[500] flex items-center justify-center bg-black/60 p-4"
            role="presentation"
            @click.self="resolveToolApproval(false)"
        >
            <section
                class="flex max-h-[85vh] w-full max-w-2xl flex-col gap-3 overflow-hidden rounded-lg border border-solid border-surface-5 bg-surface-2 p-4 shadow-xl"
                role="alertdialog"
                aria-modal="true"
                aria-labelledby="mcp-approval-title"
            >
                <h2
                    id="mcp-approval-title"
                    class="m-0 text-lg font-semibold text-[var(--color-text-primary)]"
                >
                    {{
                        formatMessage(
                            currentApproval.endpoint === 'MC-Trin'
                                ? messages.localApprovalTitle
                                : messages.mcpApprovalTitle,
                        )
                    }}
                </h2>
                <dl class="m-0 grid grid-cols-[auto_minmax(0,1fr)] gap-x-3 gap-y-2 text-sm">
                    <dt class="text-[var(--color-text-tertiary)]">
                        {{ formatMessage(messages.mcpServer) }}
                    </dt>
                    <dd class="m-0 break-all text-[var(--color-text-primary)]">
                        {{ currentApproval.endpoint }}
                    </dd>
                    <dt class="text-[var(--color-text-tertiary)]">
                        {{ formatMessage(messages.mcpTool) }}
                    </dt>
                    <dd class="m-0 break-all text-[var(--color-text-primary)]">
                        {{ currentApproval.tool.name }}
                    </dd>
                </dl>
                <p
                    v-if="currentApproval.tool.description"
                    class="m-0 break-words text-sm text-[var(--color-text-tertiary)]"
                >
                    {{ currentApproval.tool.description }}
                </p>
                <div
                    class="min-h-0 overflow-auto rounded-md border border-solid border-surface-4 bg-surface-1 p-3"
                >
                    <h3 class="mb-2 mt-0 text-sm font-semibold text-[var(--color-text-primary)]">
                        {{ formatMessage(messages.mcpArguments) }}
                    </h3>
                    <pre
                        class="m-0 whitespace-pre-wrap break-all text-xs text-[var(--color-text-primary)]"
                        >{{ JSON.stringify(currentApproval.args, null, 2) }}</pre>
                </div>
                <div class="flex flex-wrap justify-end gap-2">
                    <Button type="outlined" @click="resolveToolApproval(false)">
                        {{ formatMessage(messages.mcpDeny) }}
                    </Button>
                    <Button type="colored" color="brand" @click="resolveToolApproval(true)">
                        {{ formatMessage(messages.mcpApprove) }}
                    </Button>
                </div>
            </section>
        </div>
    </main>
</template>
