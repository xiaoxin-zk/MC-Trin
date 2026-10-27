import { flushPromises, mount } from '@vue/test-utils'
import { afterEach, beforeEach, expect, it, vi } from 'vitest'
import { defineComponent, h, ref } from 'vue'

import type { BackupProgressEvent, BackupSnapshot } from '@/helpers/instance-backup'

import BackupSettings from './BackupSettings.vue'

const fixture = vi.hoisted(() => ({
    progress: undefined as ((event: BackupProgressEvent) => void) | undefined,
    deleteBackup: vi.fn(),
    handleError: vi.fn(),
}))

vi.mock('@/providers/instance-settings', async () => {
    const { ref } = await import('vue')
    return {
        injectInstanceSettings: () => ({ instance: ref({ id: 'instance-a' }) }),
    }
})
vi.mock('@/helpers/instance', () => ({ get_full_path: async () => '/instances/instance-a' }))
vi.mock('@/helpers/instance-backup', () => ({
    backupProgressState: (stage: string) =>
        stage === 'copying' || stage === 'restoring' ? 'materializing' : stage,
    getBackupConfig: async () => ({
        instance_id: 'instance-a',
        enabled: true,
        eligibility: 'eligible',
        excluded_paths: [],
        snapshot_count: 1,
    }),
    listBackups: async () => [
        {
            id: 'snapshot-a',
            instance_id: 'instance-a',
            instance_name: 'Demo',
            created_at: 1,
            file_count: 1,
            symlink_count: 0,
            logical_size: 1024,
            added_size: 1024,
        } satisfies BackupSnapshot,
    ],
    listBackupOperations: async () => [],
    listenBackupProgress: async (handler: (event: BackupProgressEvent) => void) => {
        fixture.progress = handler
        return vi.fn()
    },
    deleteBackup: fixture.deleteBackup,
    cancelBackup: vi.fn(),
    disableBackups: vi.fn(),
    enableBackups: vi.fn(),
    updateBackupExclusions: vi.fn(),
    restoreBackup: vi.fn(),
    startBackup: vi.fn(),
    startBackupRestorePreview: vi.fn(),
    normalizeBackupExclusion: vi.fn(),
}))
vi.mock('@modrinth/assets', async () => {
    const { defineComponent, h } = await import('vue')
    const icon = defineComponent({ setup: () => () => h('span', { 'aria-hidden': 'true' }) })
    return Object.fromEntries(
        ['FileArchiveIcon', 'SaveIcon', 'TrashIcon', 'UndoIcon', 'XIcon'].map((name) => [
            name,
            icon,
        ]),
    )
})
vi.mock('@modrinth/ui', () => ({
    Admonition: defineComponent({
        setup:
            (_, { slots }) =>
            () =>
                h('div', slots.default?.()),
    }),
    Button: defineComponent({
        setup:
            (_, { attrs, slots }) =>
            () =>
                h('button', attrs, slots.default?.()),
    }),
    commonMessages: {
        cancelButton: { defaultMessage: 'Cancel' },
        retryButton: { defaultMessage: 'Retry' },
    },
    defineMessages: (messages: unknown) => messages,
    injectNotificationManager: () => ({ handleError: fixture.handleError }),
    NewModal: defineComponent({
        props: ['disableClose'],
        setup: (props, { expose, slots }) => {
            const visible = ref(false)
            expose({
                show: () => (visible.value = true),
                hide: () => {
                    if (!props.disableClose) visible.value = false
                },
            })
            return () =>
                visible.value
                    ? h('section', { role: 'dialog' }, [slots.default?.(), slots.actions?.()])
                    : null
        },
    }),
    useFormatBytes: () => (value: number) => `${value} bytes`,
    useFormatDateTime: () => () => 'snapshot date',
    useVIntl: () => ({
        formatMessage: (message: { defaultMessage: string }) => message.defaultMessage,
    }),
}))
vi.mock('./BackupExclusionSelector.vue', () => ({
    default: defineComponent({ setup: () => () => h('div') }),
}))

let wrapper: ReturnType<typeof mount> | undefined
beforeEach(() => {
    fixture.progress = undefined
    fixture.deleteBackup.mockReset()
    fixture.handleError.mockReset()
})
afterEach(() => {
    wrapper?.unmount()
    wrapper = undefined
})

it('shows the deleting stage while a snapshot delete is active', async () => {
    let finishDelete!: () => void
    fixture.deleteBackup.mockReturnValueOnce(
        new Promise<void>((resolve) => {
            finishDelete = resolve
        }),
    )
    wrapper = mount(BackupSettings)
    await flushPromises()

    await wrapper.get('button[aria-label="Delete backup"]').trigger('click')
    await wrapper
        .get('[role="dialog"]')
        .findAll('button')
        .find((button) => button.text().includes('Delete backup'))!
        .trigger('click')
    await flushPromises()
    fixture.progress?.({
        operationId: 'delete-a',
        operationType: 'delete',
        instanceId: 'instance-a',
        snapshotId: 'snapshot-a',
        stage: 'deleting',
        processedBytes: 0,
        totalBytes: 1024,
    })
    await flushPromises()

    expect(wrapper.text()).toContain('Deleting backup')
    expect(wrapper.text()).not.toContain('0 bytes of 1024 bytes')

    finishDelete()
    await flushPromises()
})
