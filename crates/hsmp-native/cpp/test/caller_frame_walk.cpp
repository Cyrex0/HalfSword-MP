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
    struct Budget { std::uint64_t window{}; unsigned emitted{}, limit{}; } budget{1000, 0, 2};
    unsigned lookups = 0;
    auto verified = [&] { ++lookups; return true; };
    check(sample(budget, 1000, verified) && sample(budget, 1999, verified), "verified samples consume the current window");
    check(budget.emitted == 2 && lookups == 2, "only verified observations are counted");
    bool all_dropped = true;
    for (unsigned i = 0; i < 128; ++i) all_dropped = !sample(budget, 1999, verified) && all_dropped;
    check(all_dropped, "hot dropped callbacks skip native verification");
    check(lookups == 2 && budget.emitted == 2, "exhausted budget performs no exact lookup");
    check(sample(budget, 2000, verified) && lookups == 3 && budget.emitted == 1, "next window verifies afresh");
    check(!sample(budget, 2001, [&] { ++lookups; return false; }) && budget.emitted == 1, "same-name wrong function cannot qualify or consume observation budget");
    check(sample(budget, 2002, verified) && lookups == 5 && budget.emitted == 2, "failed identity leaves capacity for a correct function");
    Budget disabled{2000, 0, 0};
    check(!sample(disabled, 2002, verified) && lookups == 5, "zero budget skips verification");
    check(defer_different_node(10, 20, 1000, 1999), "shared FName wrong-address candidates defer expensive lookup briefly");
    check(!defer_different_node(10, 20, 1000, 2000), "negative filter expires for relocated function");
    check(!defer_different_node(20, 20, 1000, 1001), "same recycled address must still reach exact verifier");
    check(!defer_different_node(10, 0, 1000, 1001) && !defer_different_node(10, 20, 0, 1001), "unknown lookup state is not a cached identity verdict");
    std::array<Budget, 2> ambiguous{{{3000, 0, 1}, {3000, 0, 1}}};
    unsigned candidate_lookups = 0;
    check(sample_candidates(3, ambiguous.size(), ambiguous.data(), 3001, [&](unsigned role) {
        ++candidate_lookups; return role == 2;
    }) == 2 && candidate_lookups == 2 && ambiguous[0].emitted == 0 && ambiguous[1].emitted == 1,
        "same-name first rejected candidate does not starve the correct role");
    ambiguous[0].emitted = 1; ambiguous[1].emitted = 0; candidate_lookups = 0;
    check(sample_candidates(3, ambiguous.size(), ambiguous.data(), 3002, [&](unsigned role) {
        ++candidate_lookups; return role == 2;
    }) == 2 && candidate_lookups == 1, "same-name exhausted candidate does not starve eligible role");
    check(sample_candidates(0, ambiguous.size(), ambiguous.data(), 3002, [&](auto) {
        ++candidate_lookups; return true;
    }) == 0 && candidate_lookups == 1, "no cheap candidate performs no native verification");
    std::printf("caller_frame_walk: %u checks passed\n", checks);
}
