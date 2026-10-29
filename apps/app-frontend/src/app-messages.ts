import { defineMessages } from '@modrinth/ui'

export const messages = defineMessages({
    modrinthAccount: { id: 'app.account.modrinth-account', defaultMessage: 'Modrinth account' },
    signInModrinth: {
        id: 'app.account.sign-in-modrinth',
        defaultMessage: 'Sign in to a Modrinth account',
    },
    updateInstalledToastTitle: {
        id: 'app.update.complete-toast.title',
        defaultMessage: 'Version {version} was successfully installed!',
    },
    updateInstalledToastText: {
        id: 'app.update.complete-toast.text',
        defaultMessage: 'The launcher update has been installed.',
    },
    directLinkSyncIssuesTitle: {
        id: 'app.direct-link-sync.issues-title',
        defaultMessage: 'External Minecraft instances need attention',
    },
    authUnreachableHeader: {
        id: 'app.auth-servers.unreachable.header',
        defaultMessage: 'Cannot reach authentication servers',
    },
    authUnreachableBody: {
        id: 'app.auth-servers.unreachable.body',
        defaultMessage:
            'Minecraft authentication servers may be down right now. Check your internet connection and try again later.',
    },
    quitLauncher: {
        id: 'app.initialization.quit',
        defaultMessage: 'Quit launcher',
    },
    runningAsAdmin: {
        id: 'app.warning.running-as-admin',
        defaultMessage:
            'MC-Trin is running as administrator. Drag-and-drop file import is disabled in this mode; please restart the launcher without administrator privileges.',
    },
    restarting: {
        id: 'app.restarting',
        defaultMessage: 'Restarting...',
    },
    closeLauncherTitle: {
        id: 'app.close-launcher.title',
        defaultMessage: 'Choose how to close MC-Trin',
    },
    closeLauncherDirect: {
        id: 'app.close-launcher.direct',
        defaultMessage: 'Close directly',
    },
    closeLauncherTray: {
        id: 'app.close-launcher.tray',
        defaultMessage: 'Hide to tray',
    },
    closeLauncherRemember: {
        id: 'app.close-launcher.remember',
        defaultMessage: 'Remember my choice',
    },
    backupExitTitle: {
        id: 'app.backup-exit.title',
        defaultMessage: 'Backup operations are still running',
    },
    backupExitBody: {
        id: 'app.backup-exit.body',
        defaultMessage:
            '{count, plural, one {# backup operation is} other {# backup operations are}} still running. Keep the launcher open or hide it to the tray to let them finish.',
    },
    backupExitRepositoryMoveBody: {
        id: 'app.backup-exit.repository-move-body',
        defaultMessage:
            'The backup repository is being moved. This task cannot be cancelled or force-stopped; keep the launcher open or hide it to the tray until it finishes.',
    },
    backupExitReturn: { id: 'app.backup-exit.return', defaultMessage: 'Return to launcher' },
    backupExitTray: { id: 'app.backup-exit.tray', defaultMessage: 'Hide to tray' },
    backupExitCancel: {
        id: 'app.backup-exit.cancel',
        defaultMessage: 'Cancel tasks and exit',
    },
    backupExitForce: { id: 'app.backup-exit.force', defaultMessage: 'Force exit' },
    backupExitTimeout: {
        id: 'app.backup-exit.timeout',
        defaultMessage: 'Timed out while waiting for backup operations to stop.',
    },
    home: {
        id: 'app.navigation.home',
        defaultMessage: 'Home',
    },
    worlds: {
        id: 'app.navigation.worlds',
        defaultMessage: 'Worlds',
    },
    discoverContent: {
        id: 'app.navigation.discover-content',
        defaultMessage: 'Discover content',
    },
    skinSelector: {
        id: 'app.navigation.skin-selector',
        defaultMessage: 'Skin selector',
    },
    library: {
        id: 'app.navigation.library',
        defaultMessage: 'Library',
    },
    multiplayer: {
        id: 'app.navigation.multiplayer',
        defaultMessage: 'Multiplayer',
    },
    aiAssistant: {
        id: 'app.navigation.ai-assistant',
        defaultMessage: 'AI assistant',
    },
    plugins: { id: 'app.plugins.title', defaultMessage: 'Plugins' },
    downloads: {
        id: 'app.navigation.downloads',
        defaultMessage: 'Downloads',
    },
    screenshots: {
        id: 'app.navigation.screenshots',
        defaultMessage: 'Screenshots',
    },
    lab: {
        id: 'app.navigation.lab',
        defaultMessage: 'Lab',
    },
    createInstance: {
        id: 'app.navigation.create-instance',
        defaultMessage: 'Create new instance',
    },
    signedInAs: {
        id: 'app.account.signed-in-as',
        defaultMessage: 'Signed in as',
    },
    playingAs: {
        id: 'app.minecraft.playing-as',
        defaultMessage: 'Playing as',
    },
    collapseSidebar: {
        id: 'app.sidebar.collapse',
        defaultMessage: 'Collapse sidebar',
    },
    expandSidebar: {
        id: 'app.sidebar.expand',
        defaultMessage: 'Expand sidebar',
    },
    collapseNavigation: {
        id: 'app.navigation.collapse',
        defaultMessage: 'Collapse navigation',
    },
    expandNavigation: {
        id: 'app.navigation.expand',
        defaultMessage: 'Expand navigation',
    },
    navGroupGame: {
        id: 'app.navigation.group-game',
        defaultMessage: 'Game',
    },
    navGroupSocial: {
        id: 'app.navigation.group-social',
        defaultMessage: 'Social',
    },
    navGroupAi: {
        id: 'app.navigation.group-ai',
        defaultMessage: 'AI',
    },
    navGroupExtend: {
        id: 'app.navigation.group-extend',
        defaultMessage: 'Extend',
    },
    warning: {
        id: 'app.notification.warning',
        defaultMessage: 'Warning',
    },
    exportErrorLogs: {
        id: 'app.notification.export-error-logs',
        defaultMessage: 'Export error logs',
    },
    copyPagePath: {
        id: 'app.page-path.copy',
        defaultMessage: 'Copy page path',
    },
    pagePathCopied: {
        id: 'app.page-path.copied',
        defaultMessage: 'Copied',
    },

    // ── Drop / import notification messages ──
    dropOverlayTitle: {
        id: 'app.drop.overlay-title',
        defaultMessage: 'Drop to import',
    },
    dropOverlaySubtitle: {
        id: 'app.drop.overlay-subtitle',
        defaultMessage: 'Release to analyze',
    },
    dropProcessing: {
        id: 'app.drop.processing',
        defaultMessage: 'Processing {name}...',
    },
    dropMultipleFilesTitle: {
        id: 'app.drop.error.multiple-files-title',
        defaultMessage: 'Cannot import multiple files',
    },
    dropMultipleFilesText: {
        id: 'app.drop.error.multiple-files-text',
        defaultMessage: 'Please drop one file at a time.',
    },
    dropShortcutFailedTitle: {
        id: 'app.drop.error.shortcut-title',
        defaultMessage: 'Shortcut resolution failed',
    },
    dropShortcutFailedText: {
        id: 'app.drop.error.shortcut-text',
        defaultMessage: 'Could not resolve the shortcut target.',
    },
    dropUnknownTitle: {
        id: 'app.drop.error.unknown-title',
        defaultMessage: 'Unknown file type',
    },
    dropUnknownText: {
        id: 'app.drop.error.unknown-text',
        defaultMessage: 'Could not determine what kind of file this is.',
    },
    dropUnknownDepthText: {
        id: 'app.drop.error.unknown-depth-text',
        defaultMessage:
            'The archive is nested too deeply to analyze. Unpack it to a folder and try again.',
    },
    dropUnknownEncryptedText: {
        id: 'app.drop.error.unknown-encrypted-text',
        defaultMessage: 'The archive contains encrypted files and cannot be analyzed.',
    },
    dropNestedUnpackTitle: {
        id: 'app.drop.nested-unpack-title',
        defaultMessage: 'Nested archives detected',
    },
    dropNestedUnpackText: {
        id: 'app.drop.nested-unpack-text',
        defaultMessage:
            'This archive contains nested archives ({size}) that must be unpacked to analyze. This may take some time. Continue?',
    },
    dropNestedUnpackButton: {
        id: 'app.drop.nested-unpack-button',
        defaultMessage: 'Continue analysis',
    },
    dropErrorTitle: {
        id: 'app.drop.error.title',
        defaultMessage: 'Drop error',
    },
    dropWorldImportedTitle: {
        id: 'app.drop.world-imported-title',
        defaultMessage: 'World imported',
    },
    dropWorldImportedText: {
        id: 'app.drop.world-imported-text',
        defaultMessage: 'World save has been imported successfully.',
    },
    dropContentInstalledTitle: {
        id: 'app.drop.content-installed-title',
        defaultMessage: 'Content installed',
    },
    dropContentInstalledText: {
        id: 'app.drop.content-installed-text',
        defaultMessage: 'File has been installed to the instance.',
    },
    dropInstallFailedTitle: {
        id: 'app.drop.install-failed-title',
        defaultMessage: 'Installation failed',
    },
    dropInstanceImportedTitle: {
        id: 'app.drop.instance-imported-title',
        defaultMessage: 'Instance imported',
    },
    dropInstanceImportedText: {
        id: 'app.drop.instance-imported-text',
        defaultMessage: '{name} imported successfully.',
    },
    dropImportFailedTitle: {
        id: 'app.drop.import-failed-title',
        defaultMessage: 'Import failed',
    },
    dropImportFailedText: {
        id: 'app.drop.import-failed-text',
        defaultMessage: 'Failed to import {name}: {error}',
    },
    dropNoInstances: {
        id: 'app.drop.no-instances',
        defaultMessage: 'No instances found',
    },
    dropScanning: {
        id: 'app.drop.scanning',
        defaultMessage: 'Scanning for instances',
    },
    dropScanFailed: {
        id: 'app.drop.scan-failed',
        defaultMessage: 'Failed to scan for instances',
    },
    dropExtractFailed: {
        id: 'app.drop.extract-failed',
        defaultMessage: 'Failed to extract archive',
    },
    dropProcessFailedTitle: {
        id: 'app.drop.process-failed-title',
        defaultMessage: 'Failed to process file',
    },
    dropTemporaryFileTitle: {
        id: 'app.drop.temporary-file-title',
        defaultMessage: 'Temporary file detected',
    },
    dropTemporaryFileText: {
        id: 'app.drop.temporary-file-text',
        defaultMessage:
            'The file "{file}" appears to be a temporary copy. Try dragging the file from its original folder instead of from a browser, archive, or cloud storage.',
    },
    dropImportProgressTitle: {
        id: 'app.drop.import-progress-title',
        defaultMessage: 'Importing instances…',
    },
    dropImportProgressText: {
        id: 'app.drop.import-progress-text',
        defaultMessage: '{current} / {total} instances imported',
    },
    dropImportCompletedTitle: {
        id: 'app.drop.import-completed-title',
        defaultMessage: 'Import completed',
    },
    dropImportCompletedText: {
        id: 'app.drop.import-completed-text',
        defaultMessage: 'Successfully imported {count} instances',
    },
    dropImportCompletedPartialText: {
        id: 'app.drop.import-completed-partial-text',
        defaultMessage: 'Imported {completed} of {total} instances ({failed} failed)',
    },
    dropImportCancelledTitle: {
        id: 'app.drop.batch.import-cancelled-title',
        defaultMessage: 'Import cancelled',
    },
    dropImportCancelledText: {
        id: 'app.drop.batch.import-cancelled-text',
        defaultMessage: 'Nothing was imported.',
    },
    dropBatchNothingImportableTitle: {
        id: 'app.drop.batch.nothing-importable-title',
        defaultMessage: 'Nothing to import',
    },
    dropBatchNothingImportableText: {
        id: 'app.drop.batch.nothing-importable-text',
        defaultMessage: '{count, plural, one {# file} other {# files}} could not be recognized.',
    },
    dropBatchCompletedTitle: {
        id: 'app.drop.batch.completed-title',
        defaultMessage: 'Import finished',
    },
    dropBatchCompletedText: {
        id: 'app.drop.batch.completed-text',
        defaultMessage: 'Imported {succeeded} of {total} ({failed} failed, {skipped} skipped).',
    },
    dropBatchTargetLabel: {
        id: 'app.drop.batch.target-label',
        defaultMessage: 'Select target instance for this batch',
    },
    dropBatchGroupFileLabel: {
        id: 'app.drop.batch.group-file-label',
        defaultMessage: '{count, plural, one {# file} other {# files}}: {names}',
    },

    dropModpackInstallFailed: {
        id: 'app.drop.modpack-install-failed',
        defaultMessage: 'Failed to install modpack',
    },

    dropUnknownForceAnalysisTitle: {
        id: 'app.drop.unknown-force-analysis-title',
        defaultMessage: 'Unable to identify file type',
    },
    dropUnknownForceAnalysisText: {
        id: 'app.drop.unknown-force-analysis-text',
        defaultMessage:
            'This archive needs to be extracted and deeply analyzed to determine its content type. This may take a while. Force analysis?',
    },
    dropUnknownForceAnalysisButton: {
        id: 'app.drop.unknown-force-analysis-button',
        defaultMessage: 'Force analysis',
    },
    dropUnknownForceAnalyzing: {
        id: 'app.drop.unknown-force-analyzing',
        defaultMessage: 'Force analyzing archive...',
    },
    dropUnknownForceAnalysisFailedTitle: {
        id: 'app.drop.unknown-force-analysis-failed-title',
        defaultMessage: 'Analysis failed',
    },
    dropUnknownForceAnalysisFailedText: {
        id: 'app.drop.unknown-force-analysis-failed-text',
        defaultMessage: 'Could not identify the file type even after deep analysis.',
    },

    dropInstallModTitle: {
        id: 'app.drop.mod-compatibility-title',
        defaultMessage: 'Version Mismatch',
    },
    dropInstallModWarning: {
        id: 'app.drop.mod-compatibility-warning',
        defaultMessage:
            '{file} targets {modVersion} ({modLoader}), but the instance is {instVersion} ({instLoader}).',
    },

    // Compatible mode (pre-version-isolation) import
    dropCompatibleModeTitle: {
        id: 'app.drop.compatible-mode-title',
        defaultMessage: 'This appears to be a pre-version-isolation instance',
    },
    dropCompatibleModeDesc: {
        id: 'app.drop.compatible-mode-desc',
        defaultMessage: 'Would you like to import this as a compatible mode instance?',
    },
    dropCompatibleModeImport: {
        id: 'app.drop.compatible-mode-import',
        defaultMessage: 'Compatible Import',
    },
    dropCompatibleModeOldWay: {
        id: 'app.drop.compatible-mode-old-way',
        defaultMessage: 'Import as Old Version',
    },
    dropCompatibleModeCancel: {
        id: 'app.drop.compatible-mode-cancel',
        defaultMessage: 'Cancel',
    },
    errorOccurred: {
        id: 'app.notification.error-occurred',
        defaultMessage: 'An error occurred',
    },
})
