// Read only while the engine's hook callback owns the current FFrame.
// Actual pinned UE4SS getters return references to pointer slots, not pointers.
#pragma once
#include <array>
#include <cstddef>
#include <cstdint>

namespace hsmp_caller
{
    inline constexpr std::size_t kMaxFrames = 16;
    using Getter = void** (*)(void*);
    struct Getters { Getter node{}, object{}, previous{}; };
    enum class End { Complete, MissingGetter, MissingSlot, Cycle, Depth };
    struct Walk { std::size_t count{}; End end{End::Complete}; };

    // A cheap negative filter only. It cannot qualify identity, and expires
    // within one second so a relocated native function can be checked anew.
    inline bool defer_different_node(std::uint64_t node, std::uint64_t resolved,
                                     std::uint64_t looked, std::uint64_t now)
    {
        return resolved && node != resolved && looked && now - looked < 1000;
    }

    // Cheap eligibility precedes the native identity verifier. A failed exact
    // identity never consumes the observation budget or qualifies a role.
    template <class Budget, class Verify>
    bool sample(Budget& budget, std::uint64_t now, Verify verify)
    {
        if (now - budget.window >= 1000) { budget.window = now; budget.emitted = 0; }
        if (budget.emitted >= budget.limit) return false;
        if (!verify()) return false;
        ++budget.emitted;
        return true;
    }

    template <class Budget, class Verify>
    unsigned sample_candidates(std::uint32_t candidates, std::size_t count,
                               Budget* budgets, std::uint64_t now, Verify verify)
    {
        for (std::size_t i = 0; i < count && i < 32; ++i)
        {
            if (!(candidates & (1u << i))) continue;
            const unsigned role = static_cast<unsigned>(i + 1);
            if (sample(budgets[i], now, [&] { return verify(role); })) return role;
        }
        return 0;
    }

    // No frame or object pointer escapes except synchronously to visit().
    template <class Visitor>
    Walk walk(void* frame, const Getters& g, Visitor visit)
    {
        if (!g.node || !g.object || !g.previous) return {0, End::MissingGetter};
        std::array<void*, kMaxFrames> seen{};
        std::size_t n = 0;
        while (frame && n < kMaxFrames)
        {
            for (std::size_t i = 0; i < n; ++i)
                if (seen[i] == frame) return {n, End::Cycle};
            seen[n] = frame;
            void** node = g.node(frame);
            void** object = g.object(frame);
            void** previous = g.previous(frame);
            if (!node || !object || !previous) return {n, End::MissingSlot};
            visit(n, *node, *object);
            ++n;
            frame = *previous;
        }
        return {n, frame ? End::Depth : End::Complete};
    }
}
