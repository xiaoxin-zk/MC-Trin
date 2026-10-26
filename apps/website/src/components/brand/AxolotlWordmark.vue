<template>
    <span
        class="inline-flex items-center gap-2.5 text-[1.375rem] font-extrabold leading-none tracking-[-0.035em] text-[var(--color-contrast)]"
    >
        <span class="brand-mark" aria-hidden="true"><i /><i /><i /><i /></span>
        <span>MC-Trin</span>
    </span>
</template>

<style scoped lang="scss">
// Four-tile gradient mark, matching the launcher's brand block.
.brand-mark {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 2px;
    width: 2rem;
    height: 2rem;
    padding: 4px;
    border-radius: 9px;
    background: linear-gradient(
        145deg,
        var(--color-brand),
        color-mix(in srgb, var(--color-brand) 55%, #7dd3fc)
    );
    box-shadow: 0 0.35rem 0.7rem rgb(0 0 0 / 30%);
    transform: rotate(-6deg);

    i {
        border-radius: 2px;
        background: rgb(255 255 255 / 92%);
    }

    i:nth-child(2),
    i:nth-child(3) {
        opacity: 0.75;
    }

    i:nth-child(4) {
        opacity: 0.55;
    }
}
</style>
