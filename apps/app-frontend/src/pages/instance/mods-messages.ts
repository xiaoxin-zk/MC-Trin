import { defineMessages } from '@modrinth/ui'

export const messages = defineMessages({
    loadingOnlineInformation: {
        id: 'app.instance.mods.loading-online-information',
        defaultMessage: 'Loading online information, please wait',
    },
    versionsFailed: {
        id: 'app.instance.content.versions-failed',
        defaultMessage:
            'The release list could not be loaded. Reopen the version selector and try again.',
    },
    manageBackups: {
        id: 'app.instance.mods.manage-backups',
        defaultMessage: 'Old files and backups',
    },
    shareTitle: {
        id: 'app.instance.mods.share-title',
        defaultMessage: 'Sharing modpack content',
    },
    shareText: {
        id: 'app.instance.mods.share-text',
        defaultMessage: "Check out the projects I'm using in my modpack!",
    },
    openInSchematicWorkshop: {
        id: 'instance.files.open-in-schematic-workshop',
        defaultMessage: 'Open in schematic workshop',
    },
    successfullyUploaded: {
        id: 'app.instance.mods.successfully-uploaded',
        defaultMessage: 'Successfully uploaded',
    },
    projectWasAdded: {
        id: 'app.instance.mods.project-was-added',
        defaultMessage: '"{name}" was added',
    },
    projectsWereAdded: {
        id: 'app.instance.mods.projects-were-added',
        defaultMessage: '{count} projects were added',
    },
    orphanedDependenciesTitle: {
        id: 'app.instance.mods.orphaned-dependencies.title',
        defaultMessage: 'Dependencies are no longer required',
    },
    orphanedDependenciesBody: {
        id: 'app.instance.mods.orphaned-dependencies.body',
        defaultMessage:
            'These dependencies are no longer required by other content and will not be removed automatically: {list}',
    },
    contentTypeProject: {
        id: 'app.instance.mods.content-type-project',
        defaultMessage: 'project',
    },
    bulkUpdateResolvingVersions: {
        id: 'app.instance.mods.bulk-update.resolving-versions',
        defaultMessage: 'Resolving versions...',
    },
    bulkUpdateDownloadingProjects: {
        id: 'app.instance.mods.bulk-update.downloading-projects',
        defaultMessage: 'Downloading {current, number}/{total, number} projects...',
    },
    bulkUpdateFinishing: {
        id: 'app.instance.mods.bulk-update.finishing',
        defaultMessage: 'Finishing update...',
    },
    updateAddedContent: {
        id: 'app.instance.mods.update-added-content',
        defaultMessage: 'Update added content',
    },
    updateAddedContentDescription: {
        id: 'app.instance.mods.update-added-content.description',
        defaultMessage:
            'Updates only content added after the modpack was installed. Modpack files are not changed.',
    },
    contentOperationTimedOut: {
        id: 'app.instance.mods.content-operation-timeout',
        defaultMessage: 'Timed out waiting for another content operation to finish. Try again.',
    },
    contentRefreshWarningTitle: {
        id: 'app.instance.mods.content-refresh-warning.title',
        defaultMessage: 'Some online details are unavailable',
    },
    contentRefreshWarningBody: {
        id: 'app.instance.mods.content-refresh-warning.body',
        defaultMessage:
            'Local content is still complete and can be managed. Names, icons, or update information may be temporarily unavailable.',
    },
    contentHistoryWarningTitle: {
        id: 'app.instance.mods.content-history-warning.title',
        defaultMessage: 'Some local history details are unavailable',
    },
    packReconciliationWarningTitle: {
        id: 'app.instance.mods.pack-reconciliation-warning.title',
        defaultMessage: 'Modpack file information could not be refreshed',
    },
    packReconciliationWarningBody: {
        id: 'app.instance.mods.pack-reconciliation-warning.body',
        defaultMessage:
            'Existing files and ownership have been kept. Retry refreshing to verify missing or manually downloaded modpack files.',
    },
    contentHistoryWarningBody: {
        id: 'app.instance.mods.content-history-warning.body',
        defaultMessage:
            'Local content is still complete and can be managed. Rollback or cleanup details may be temporarily unavailable.',
    },
    skippedFilesWarningTitle: {
        id: 'app.instance.mods.skipped-files-warning.title',
        defaultMessage: 'Some modpack files require manual installation',
    },
    skippedFilesWarningBody: {
        id: 'app.instance.mods.skipped-files-warning.body',
        defaultMessage:
            '{count, plural, one {# file was} other {# files were}} skipped during installation. Select a file to download it; the monitored import folder is verified automatically. You can also drag the downloaded file into this instance.',
    },
    skippedFilesWarningBodyDisabled: {
        id: 'app.instance.mods.skipped-files-warning.body-disabled',
        defaultMessage:
            '{count, plural, one {# file was} other {# files were}} skipped during installation. Drag the downloaded file into this instance, or enable automatic import in Resource Management settings.',
    },
    skippedFilesWarningMore: {
        id: 'app.instance.mods.skipped-files-warning.more',
        defaultMessage: 'And {count, number} more.',
    },
    completeSkippedFiles: {
        id: 'app.instance.mods.skipped-files-warning.complete',
        defaultMessage: 'Complete missing files',
    },
    missingFilesWarningTitle: {
        id: 'app.instance.mods.missing-files-warning.title',
        defaultMessage: 'Some modpack files are missing',
    },
    missingFilesWarningBody: {
        id: 'app.instance.mods.missing-files-warning.body',
        defaultMessage:
            'The required files below were not downloaded or are no longer present. Restore them individually; refreshing scans the instance folder again.',
    },
    restoreMissingFile: {
        id: 'app.instance.mods.missing-files-warning.restore',
        defaultMessage: 'Restore',
    },
    parsingFiles: {
        id: 'app.instance.mods.parsing-files',
        defaultMessage: 'Parsing... larger files may take a while',
    },
    dragDropHint: {
        id: 'app.instance.mods.drag-drop-hint',
        defaultMessage: 'Release the file to install it into the current instance',
    },
    parseFailed: {
        id: 'app.instance.mods.parse-failed',
        defaultMessage: 'Failed to parse. This may not be a modpack',
    },
    fileAlreadyExists: {
        id: 'app.instance.mods.file-already-exists',
        defaultMessage: 'Failed to add. The file may already exist',
    },
    dismiss: {
        id: 'app.instance.mods.dismiss',
        defaultMessage: 'Got it',
    },
    restorePackDefault: {
        id: 'app.instance.mods.restore-pack-default',
        defaultMessage: 'Restore modpack default',
    },
    packMemberMissing: {
        id: 'app.instance.mods.pack-member-missing',
        defaultMessage: 'This modpack file is missing locally',
    },
    packMemberRemoved: {
        id: 'app.instance.mods.pack-member-removed',
        defaultMessage: 'This modpack file was removed locally',
    },
    postUpgradeNoticeTitle: {
        id: 'app.instance.mods.post-upgrade-notice.title',
        defaultMessage: 'Post-upgrade compatibility review',
    },
    postUpgradeNoticeBody: {
        id: 'app.instance.mods.post-upgrade-notice.body',
        defaultMessage:
            'Some preserved content may not support the current Minecraft version and should be reviewed manually.',
    },
    ignoreAllPostUpgradeWarnings: {
        id: 'app.instance.mods.post-upgrade-notice.ignore-all',
        defaultMessage: 'Ignore all',
    },
    postUpgradeWarningTooltip: {
        id: 'app.instance.mods.post-upgrade-notice.item-tooltip',
        defaultMessage:
            'This content was preserved during the instance upgrade without installing a new compatible version. It may be incompatible with the current Minecraft version.',
    },
    compatibilityWarning: {
        id: 'app.resources.installed.compatibility',
        defaultMessage:
            'Minecraft: {game, select, compatible {compatible} incompatible {incompatible} other {no supported version declaration}}; loader: {loader, select, compatible {compatible} incompatible {incompatible} other {no loader declaration}}.',
    },
    openInMcmod: {
        id: 'app.project.open-in-mcmod',
        defaultMessage: 'Open in MC Mod',
    },
})
