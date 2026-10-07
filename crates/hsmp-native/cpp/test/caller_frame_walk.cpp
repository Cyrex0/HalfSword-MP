#include "caller_frame_walk.hpp"
#include <array>
#include <cstdio>
#include <cstdlib>

namespace
{
    struct Frame { void* node{}; void* object{}; void* previous{}; };
    void** node(void* p) { return &static_cast<Frame*>(p)->node; }
    void** object(void* p) { return &static_cast<Frame*>(p)->object; }
    void** previous(void* p) { return &static_cast<Frame*>(p)->previous; }
    void** missing(void*) { return nullptr; }
    unsigned checks = 0;
    void check(bool ok, const char* why)
    {
        ++checks;
        if (!ok) { std::fprintf(stderr, "FAIL %s\n", why); std::exit(1); }
    }
}
int main()
{
    using namespace hsmp_caller;
    const Getters getters{node, object, previous};
    int target = 1, constraint = 2, fn_damage = 3, fn_constraint = 4;
    Frame parent{&fn_constraint, &constraint, nullptr};
    Frame current{&fn_damage, &target, &parent};
    unsigned visits = 0;
    auto result = walk(&current, getters, [&](std::size_t i, void* n, void* o) {
        check(n == (i == 0 ? static_cast<void*>(&fn_damage) : &fn_constraint), "dereferenced native Node slot");
        check(o == (i == 0 ? static_cast<void*>(&target) : &constraint), "caller Object differs from target Context");
        ++visits;
    });
    check(result.end == End::Complete && result.count == 2 && visits == 2, "complete live ancestry");
    check(walk(nullptr, getters, [](auto, auto, auto) {}).count == 0, "null root is empty");
    check(walk(&current, {}, [](auto, auto, auto) {}).end == End::MissingGetter, "missing getter fails closed");
    check(walk(&current, {node, missing, previous}, [](auto, auto, auto) {}).end == End::MissingSlot, "missing native slot is unavailable");
    parent.previous = &current;
    check(walk(&current, getters, [](auto, auto, auto) {}).end == End::Cycle, "cycle is bounded");
    std::array<Frame, kMaxFrames + 1> long_chain{};
    for (std::size_t i = 0; i + 1 < long_chain.size(); ++i) long_chain[i].previous = &long_chain[i + 1];
    auto bounded = walk(long_chain.data(), getters, [](auto, auto, auto) {});
    check(bounded.end == End::Depth && bounded.count == 16, "deep ancestry is explicitly incomplete");
    Frame empty{};
    bool observed_null = false;
    auto null_value = walk(&empty, getters, [&](auto, void* n, void* o) { observed_null = !n && !o; });
    check(null_value.end == End::Complete && observed_null, "readable null value differs from unreadable slot");
    std::printf("caller_frame_walk: %u checks passed\n", checks);
}
