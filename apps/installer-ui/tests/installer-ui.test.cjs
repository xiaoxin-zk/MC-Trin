const assert = require('node:assert/strict')
const { test } = require('node:test')
const path = require('node:path')
const { createRequire } = require('node:module')
const { pathToFileURL } = require('node:url')
const frontendRequire = createRequire(path.resolve(__dirname, '../../app-frontend/package.json'))
const { chromium } = frontendRequire('playwright')
const pageUrl = pathToFileURL(path.resolve(__dirname, '../src/installer.html')).href
const logoUrl = pathToFileURL(path.resolve(__dirname, '../../app/icons/128x128.png')).href
const installDir = String.raw`D:\Program Files\游戏\MC-Trin`

async function openPage(browser, mode) {
	const page = await browser.newPage({
		viewport: { width: 940, height: 620 },
		reducedMotion: 'reduce',
	})
	const errors = []
	page.on('pageerror', (error) => errors.push(error.message))
	await page.addInitScript(
		(boot) => {
			window.__MCTRIN_INSTALLER__ = boot
			window.testCommands = []
			window.ipc = { postMessage: (value) => window.testCommands.push(JSON.parse(value)) }
		},
		{
			language: 'en',
			version: '1.9.7',
			installDir,
			resourceDir: String.raw`D:\Minecraft`,
			logoDataUrl: logoUrl,
			freshInstall: true,
			...mode,
		},
	)
	await page.goto(pageUrl)
	return { page, errors }
}

async function checkBounds(page) {
	await page.locator('.screen.active').evaluate(async (element) => {
		await Promise.all(
			element
				.getAnimations({ subtree: true })
				.filter((animation) => animation.effect.getComputedTiming().iterations !== Infinity)
				.map((animation) => animation.finished.catch(() => {})),
		)
	})
	const outside = await page.evaluate(() => {
		const footer = document.querySelector('.footer').getBoundingClientRect()
		return [
			...document.querySelectorAll(
				'.screen.active h1, .screen.active h2, .screen.active p, .screen.active .fact-value, .screen.active .checkbox-label, .screen.active .error-panel.visible, .footer button:not(.hidden)',
			),
		]
			.filter((element) => {
				const r = element.getBoundingClientRect()
				return (
					r.width > 0 &&
					(r.left < 0 ||
						r.right > innerWidth ||
						r.top < 54 ||
						r.bottom > innerHeight ||
						(element.closest('.screen') && r.bottom > footer.top))
				)
			})
			.map((element) => element.textContent)
	})
	assert.deepEqual(outside, [], 'Content should stay inside the window and above the footer')
	const horizontalOverflow = await page.evaluate(() =>
		[
			document.documentElement,
			document.body,
			...document.querySelectorAll(
				'.screen.active, .screen.active .screen-content-wrapper, .screen.active h1, .screen.active h2, .screen.active p',
			),
		]
			.filter((element) => element.scrollWidth > element.clientWidth + 1)
			.map((element) => ({ tag: element.tagName, text: element.textContent.trim() })),
	)
	assert.deepEqual(
		horizontalOverflow,
		[],
		'Text should fit without clipping or horizontal scrolling',
	)
}

for (const language of ['en', 'zhCn']) {
	test('uninstall confirmation, progress, failure, retry and finish: ' + language, async () => {
		const browser = await chromium.launch({ headless: true })
		try {
			const { page, errors } = await openPage(browser, { language, uninstall: true })
			assert.equal(
				await page.locator('[data-screen="uninstall"]').getAttribute('class'),
				'screen active',
			)
			assert.equal(await page.locator('#delete-app-data').isChecked(), false)
			assert.equal(await page.locator('#uninstall-directory').textContent(), installDir)
			assert.equal(
				await page.locator('#primary-label').textContent(),
				language === 'en' ? 'Uninstall' : '卸载',
			)
			assert.equal(
				await page.locator('#secondary-action').textContent(),
				language === 'en' ? 'Cancel' : '取消',
			)
			assert.equal(
				await page
					.locator('[data-screen="uninstall"] h2')
					.evaluate((element) => getComputedStyle(element).fontSize),
				'28px',
			)
			await checkBounds(page)
			await page.locator('#secondary-action').click()
			assert.deepEqual(await page.evaluate(() => window.testCommands.pop()), { command: 'close' })
			await page.locator('#delete-app-data-row').click()
			await page.locator('#primary-action').click()
			assert.deepEqual(await page.evaluate(() => window.testCommands.pop()), {
				command: 'uninstall',
				deleteAppData: true,
			})
			assert.equal(await page.locator('#primary-action').isDisabled(), true)
			assert.equal(await page.locator('#close').isDisabled(), true)
			await page.evaluate(() => window.mctrinInstaller.receive({ type: 'installStarted' }))
			assert.equal(await page.locator('#install-summary').textContent(), installDir)
			assert.equal(await page.locator('#resource-summary-row').isVisible(), false)
			for (const progress of [4, 26, 66, 82]) {
				await page.evaluate(
					(value) => window.mctrinInstaller.receive({ type: 'progress', value }),
					progress,
				)
				await checkBounds(page)
			}
			await page.evaluate(() =>
				window.mctrinInstaller.receive({
					type: 'installFailed',
					exitCode: 2,
					message: 'Could not remove a locked file',
				}),
			)
			assert.equal(await page.locator('#primary-action').isDisabled(), false)
			assert.equal(await page.locator('#close').isDisabled(), false)
			assert.equal(await page.locator('.spinner').isVisible(), false)
			assert.equal(
				await page
					.locator('body')
					.evaluate((element) => element.classList.contains('is-installing')),
				false,
			)
			assert.equal(
				await page.locator('#progress-title').textContent(),
				language === 'en' ? 'Uninstallation failed' : '卸载失败',
			)
			assert.match(await page.locator('#install-error').textContent(), /locked file/)
			await checkBounds(page)
			await page.locator('#primary-action').click()
			assert.deepEqual(await page.evaluate(() => window.testCommands.pop()), {
				command: 'uninstall',
				deleteAppData: true,
			})
			assert.equal(await page.locator('#install-error').isVisible(), false)
			assert.equal(await page.locator('.spinner').isVisible(), true)
			await page.evaluate(() =>
				window.mctrinInstaller.receive({ type: 'installFinished', launchAfter: true }),
			)
			assert.equal(
				await page.locator('#primary-label').textContent(),
				language === 'en' ? 'Close' : '关闭',
			)
			await checkBounds(page)
			await page.locator('#primary-action').click()
			assert.deepEqual(await page.evaluate(() => window.testCommands.pop()), {
				command: 'finish',
				launch: false,
			})
			assert.deepEqual(errors, [])
			await page.close()
		} finally {
			await browser.close()
		}
	})
}

for (const language of ['en', 'zhCn']) {
	test('installation layout and actions for first install and update: ' + language, async () => {
		const browser = await chromium.launch({ headless: true })
		try {
			for (const freshInstall of [true, false]) {
				const { page, errors } = await openPage(browser, {
					language,
					uninstall: false,
					freshInstall,
				})
				await checkBounds(page)
				await page.locator('#primary-action').click()
				assert.equal(
					await page.locator('[data-screen="configure"]').getAttribute('class'),
					'screen active',
				)
				assert.equal(
					await page.locator('#primary-label').textContent(),
					language === 'en'
						? freshInstall
							? 'Install'
							: 'Update'
						: freshInstall
							? '安装'
							: '更新',
				)
				assert.equal(await page.locator('#resource-field').isVisible(), freshInstall)
				await checkBounds(page)
				await page.locator('#primary-action').click()
				const request = await page.evaluate(() => window.testCommands.pop())
				assert.equal(request.command, 'install')
				assert.equal(request.installDir, installDir)
				assert.equal(request.launchAfter, true)
				await page.evaluate(() => window.mctrinInstaller.receive({ type: 'installStarted' }))
				for (const value of [4, 26, 66, 82]) {
					await page.evaluate(
						(value) => window.mctrinInstaller.receive({ type: 'progress', value }),
						value,
					)
					await checkBounds(page)
				}
				await page.evaluate(() =>
					window.mctrinInstaller.receive({
						type: 'installFailed',
						exitCode: 2,
						message: 'Could not write launcher files. Close MC-Trin and retry.',
					}),
				)
				await checkBounds(page)
				await page.locator('#primary-action').click()
				assert.equal(await page.locator('#install-error').isVisible(), false)
				await page.evaluate(() =>
					window.mctrinInstaller.receive({ type: 'installFinished', launchAfter: true }),
				)
				await checkBounds(page)
				await page.locator('#primary-action').click()
				assert.deepEqual(await page.evaluate(() => window.testCommands.pop()), {
					command: 'finish',
					launch: true,
				})
				assert.deepEqual(errors, [])
				await page.close()
			}
		} finally {
			await browser.close()
		}
	})
}

test('cancelled UAC request restores uninstall controls without reporting success', async () => {
	const browser = await chromium.launch({ headless: true })
	try {
		const { page, errors } = await openPage(browser, { uninstall: true })
		await page.locator('#primary-action').click()
		assert.equal(await page.locator('#primary-action').isDisabled(), true)
		await page.evaluate(() =>
			window.mctrinInstaller.receive({
				type: 'installFailed',
				exitCode: null,
				message: 'The operation was canceled by the user. (os error 1223)',
			}),
		)
		assert.equal(await page.locator('#primary-action').isDisabled(), false)
		assert.equal(await page.locator('#close').isDisabled(), false)
		assert.equal(await page.locator('[data-screen="complete"]').getAttribute('class'), 'screen')
		assert.match(await page.locator('#install-error').textContent(), /canceled/)
		assert.equal(await page.locator('.spinner').isVisible(), false)
		assert.deepEqual(errors, [])
	} finally {
		await browser.close()
	}
})

test('long uninstall failure details scroll vertically while actions remain available', async () => {
	const browser = await chromium.launch({ headless: true })
	try {
		for (const language of ['en', 'zhCn']) {
			const { page, errors } = await openPage(browser, { uninstall: true, language })
			await page.evaluate(() => window.mctrinInstaller.receive({ type: 'installStarted' }))
			await checkBounds(page)
			const footerBefore = await page.locator('.footer').boundingBox()
			await page.evaluate(() =>
				window.mctrinInstaller.receive({
					type: 'installFailed',
					exitCode: 2,
					message:
						'Could not access the following file: ' +
						'nested-directory/'.repeat(80) +
						'launcher.exe. ' +
						'Close the launcher and check directory permissions. '.repeat(40),
				}),
			)
			const dimensions = await page.locator('.screen.active').evaluate((screen) => ({
				width: screen.clientWidth,
				scrollWidth: screen.scrollWidth,
				height: screen.clientHeight,
				scrollHeight: screen.scrollHeight,
				top: screen.getBoundingClientRect().top,
				contentTop: screen.querySelector('.screen-content-wrapper').getBoundingClientRect().top,
			}))
			assert.ok(
				dimensions.scrollHeight > dimensions.height,
				'Long diagnostics should be scrollable',
			)
			assert.ok(
				dimensions.contentTop >= dimensions.top,
				'The start of the content should remain reachable',
			)
			assert.ok(dimensions.scrollWidth <= dimensions.width + 1, 'Long file paths should wrap')
			await page.locator('.screen.active').evaluate((screen) => {
				screen.scrollTop = screen.scrollHeight
			})
			const errorBottom = await page
				.locator('#install-error')
				.evaluate((element) => element.getBoundingClientRect().bottom)
			assert.ok(
				errorBottom <= footerBefore.y,
				'The end of the diagnostic should be reachable above the footer',
			)
			assert.deepEqual(await page.locator('.footer').boundingBox(), footerBefore)
			assert.equal(await page.locator('#primary-action').isEnabled(), true)
			assert.equal(await page.locator('#close').isEnabled(), true)
			assert.deepEqual(errors, [])
			await page.close()
		}
	} finally {
		await browser.close()
	}
})

const previewDir = String.raw`D:\mc\MC-trin`

test('the data folder preview follows every choice that changes it', async () => {
	const browser = await chromium.launch({ headless: true })
	try {
		const { page, errors } = await openPage(browser, { freshInstall: true })
		await page.locator('#primary-action').click()

		// Browse assigns the field programmatically, which fires no input event,
		// so the preview has to be refreshed by the handler itself.
		await page.evaluate(
			(dir) =>
				window.mctrinInstaller.receive({
					type: 'pathSelected',
					target: 'install',
					path: dir,
				}),
			previewDir,
		)
		assert.equal(
			await page.locator('#data-path').textContent(),
			String.raw`D:\mc\MC-trin\data`,
		)

		// Typing into the field does fire input, and must update it too.
		await page.locator('#install-dir').fill(String.raw`E:\Games\MC`)
		assert.equal(
			await page.locator('#data-path').textContent(),
			String.raw`E:\Games\MC\data`,
		)

		// A portable install keeps everything beside the program.
		await page.locator('#portable-mode').check()
		assert.equal(await page.locator('#data-path').textContent(), String.raw`E:\Games\MC`)

		// Trailing separators are normalised rather than doubled, including a
		// run of them, and the submitted folder is normalised the same way.
		await page.locator('#portable-mode').uncheck()
		await page.locator('#install-dir').fill('D:\\mc\\MC-trin\\\\')
		assert.equal(
			await page.locator('#data-path').textContent(),
			String.raw`D:\mc\MC-trin\data`,
		)
		await page.locator('#portable-mode').check()
		assert.equal(await page.locator('#data-path').textContent(), previewDir)

		// Forward slashes are folded to backslashes, and a trailing one is
		// dropped, so the preview is spelled the way NSIS canonicalises the path
		// instead of mixing both separators into `D:/mc/MC-trin\data`.
		await page.locator('#portable-mode').uncheck()
		await page.locator('#install-dir').fill('D:/mc/MC-trin/')
		assert.equal(
			await page.locator('#data-path').textContent(),
			String.raw`D:\mc\MC-trin\data`,
		)
		// The submitted folder uses the same canonical spelling, so the request
		// cannot disagree with what the preview promised.
		await page.locator('#primary-action').click()
		const slashRequest = await page.evaluate(() => window.testCommands.pop())
		assert.equal(slashRequest.command, 'install')
		assert.equal(slashRequest.installDir, previewDir)

		// A drive root keeps its separator instead of collapsing to `D:`.
		await page.locator('#portable-mode').uncheck()
		await page.locator('#install-dir').fill('D:\\')
		assert.equal(await page.locator('#data-path').textContent(), 'D:\\data')

		// UNC separators are folded too, and the share root stays intact.
		await page.locator('#install-dir').fill('//server/share/')
		assert.equal(
			await page.locator('#data-path').textContent(),
			String.raw`\\server\share\data`,
		)
		await page.locator('#primary-action').click()
		const uncRequest = await page.evaluate(() => window.testCommands.pop())
		assert.equal(uncRequest.command, 'install')
		assert.equal(uncRequest.installDir, String.raw`\\server\share`)

		await page.locator('#install-dir').fill(String.raw`D:\mc\MC-trin\\`)
		await page.locator('#primary-action').click()
		const request = await page.evaluate(() => window.testCommands.pop())
		assert.equal(request.command, 'install')
		assert.equal(request.installDir, previewDir)
		assert.deepEqual(errors, [])
		await page.close()
	} finally {
		await browser.close()
	}
})

test('an update derives the recorded layout and cannot move its folder', async () => {
	const browser = await chromium.launch({ headless: true })
	try {
		const portable = await openPage(browser, {
			freshInstall: false,
			portable: true,
			installDir: previewDir,
		})
		await portable.page.locator('#primary-action').click()
		assert.equal(await portable.page.locator('#portable-mode').isChecked(), true)
		assert.equal(await portable.page.locator('#data-path').textContent(), previewDir)
		// An update installs over the folder NSIS detected the layout for, so
		// the preview and the request cannot drift to a different one.
		assert.equal(await portable.page.locator('#install-dir').isEditable(), false)
		assert.equal(await portable.page.locator('#browse-install').isDisabled(), true)
		await portable.page.locator('#primary-action').click()
		const portableRequest = await portable.page.evaluate(() => window.testCommands.pop())
		assert.equal(portableRequest.installDir, previewDir)
		assert.equal(portableRequest.portable, true)
		assert.deepEqual(portable.errors, [])
		await portable.page.close()

		const normal = await openPage(browser, {
			freshInstall: false,
			portable: false,
			installDir: previewDir,
		})
		await normal.page.locator('#primary-action').click()
		assert.equal(await normal.page.locator('#portable-mode').isChecked(), false)
		assert.equal(
			await normal.page.locator('#data-path').textContent(),
			String.raw`D:\mc\MC-trin\data`,
		)
		assert.deepEqual(normal.errors, [])
		await normal.page.close()
	} finally {
		await browser.close()
	}
})
