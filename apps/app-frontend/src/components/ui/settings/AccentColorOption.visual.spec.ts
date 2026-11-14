import '@modrinth/ui/directives/tooltip-styles.ts'

import { tooltipDirective } from '@modrinth/ui/directives/tooltip.ts'
import { afterEach, expect, it, vi } from 'vitest'

import { mountThemed } from '@/test/visual-harness'

import AccentColorOption from './AccentColorOption.vue'

const cleanup: (() => void)[] = []
afterEach(() => cleanup.splice(0).forEach((fn) => fn()))

it('keeps the label clear of the corner check and keeps selection geometry stable in narrow regions', async () => {
    for (const label of ['Pink', '自定义强调色', 'Follow the operating system accent color']) {
        const wrapper = await mountThemed(
            AccentColorOption,
            { label, background: '#ff66aa', selected: false, style: { width: '108px' } },
            'dark',
            { global: { directives: { tooltip: tooltipDirective } } },
        )
        cleanup.push(() => wrapper.unmount())
        const button = wrapper.element as HTMLElement
        const text = button.children[1] as HTMLElement
        const check = button.querySelector('svg')!
        const before = [
            text.getBoundingClientRect().width,
            check.getBoundingClientRect().left,
            button.getBoundingClientRect().height,
        ]
        const buttonRect = button.getBoundingClientRect()
        const textRect = text.getBoundingClientRect()
        const checkRect = check.getBoundingClientRect()

        // The check is taken out of the flow, so the label column is the only
        // track and the button's right padding is what keeps text off the check.
        expect(checkRect.left).toBeGreaterThanOrEqual(textRect.right)
        expect(checkRect.right).toBeLessThanOrEqual(buttonRect.right)
        expect(checkRect.top).toBeLessThan(textRect.top + textRect.height / 2)
        expect(getComputedStyle(check).opacity).toBe('0')
        await wrapper.setProps({ selected: true })
        expect(getComputedStyle(check).opacity).toBe('1')
        expect([
            text.getBoundingClientRect().width,
            check.getBoundingClientRect().left,
            button.getBoundingClientRect().height,
        ]).toEqual(before)
        expect(wrapper.attributes('aria-checked')).toBe('true')
    }
})

it('keeps the system description and status separate and preserves disabled activation', async () => {
    const onClick = vi.fn()
    const wrapper = await mountThemed(
        AccentColorOption,
        {
            label: 'System',
            accessibleLabel: 'System accent unavailable',
            description: 'Unavailable on this system',
            background: 'var(--color-pink)',
            selected: false,
            disabled: true,
            onClick,
            style: { width: '168px' },
        },
        'dark',
        { global: { directives: { tooltip: tooltipDirective } } },
    )
    cleanup.push(() => wrapper.unmount())
    const button = wrapper.element as HTMLButtonElement
    expect(button.disabled).toBe(true)
    button.click()
    expect(onClick).not.toHaveBeenCalled()
    expect(button.getAttribute('aria-label')).toBe('System accent unavailable')
    expect(button.children[1].getBoundingClientRect().right).toBeLessThan(
        button.querySelector('svg')!.getBoundingClientRect().left,
    )
    await wrapper.setProps({ disabled: false })
    button.click()
    expect(onClick).toHaveBeenCalledTimes(1)
})
