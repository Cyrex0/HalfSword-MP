// Read only while the engine's hook callback owns the current FFrame.
// Actual pinned UE4SS getters return references to pointer slots, not pointers.
#pragma once
#include <array>
#include <cstddef>

namespace hsmp_caller
{
    inline constexpr std::size_t kMaxFrames = 16;
    using Getter = void** (*)(void*);
    struct Getters { Getter node{}, object{}, previous{}; };
    enum class End { Complete, MissingGetter, MissingSlot, Cycle, Depth };
    struct Walk { std::size_t count{}; End end{End::Complete}; };

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
