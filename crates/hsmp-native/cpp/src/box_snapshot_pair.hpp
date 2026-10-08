// Bounded proof-only native entry/post pairing. Contains scalar copies only.
#pragma once
#include <array>
#include <cmath>
#include <cstdint>
#include <cstddef>

namespace hsmp_box
{
inline constexpr unsigned kMaxPairs = 32, kMaxDepth = 8;
inline constexpr std::uint64_t kMaxDurationMs = 15000;
enum class Reason : unsigned
{
    None, Off, BadInput, Unavailable, Thread, Expired, Budget, Depth, Ordering,
    Identity, World, Params, Extent, Lifetime, NoLuaMark, Scope, Submission
};
inline const char* reason_name(Reason r)
{
    constexpr const char* names[] = {"ok","off","bad input","unavailable","game thread unavailable","expired",
        "capture budget","nesting overflow","unpaired ordering","identity unavailable","world changed",
        "formal parameters unavailable","extent unavailable","lifetime unavailable","Lua POST not inside entry",
        "scope changed","callback submission uncertain"};
    const auto n = static_cast<unsigned>(r);
    return n < sizeof(names)/sizeof(names[0]) ? names[n] : "unavailable";
}
struct Identity
{
    std::uint64_t address{}, weak{}, name{}, class_address{}, class_weak{}, class_name{};
    bool operator==(const Identity&) const = default;
    bool present() const { return address && weak && name && class_address && class_weak && class_name; }
    bool persistent() const
    {
        const auto serial=weak>>32, class_serial=class_weak>>32;
        return present() && serial>0 && serial<=INT32_MAX && class_serial>0 && class_serial<=INT32_MAX;
    }
};
struct Scope
{
    Identity world{}, pawn{}, mesh{}, box{}, box_owner{};
    std::uint64_t match_id{};
    std::uint32_t round{}, life{};
    bool operator==(const Scope&) const = default;
    bool present() const { return match_id && round && life && world.present() && pawn.present()
        && mesh.present() && box.present() && box_owner.present(); }
    bool persistent() const { return world.persistent() && pawn.persistent() && mesh.persistent()
        && box.persistent() && box_owner.persistent(); }
};
struct Extent
{
    double x{}, y{}, z{};
    bool valid() const { return std::isfinite(x) && std::isfinite(y) && std::isfinite(z)
        && x >= 0 && y >= 0 && z >= 0; }
};
struct Snapshot
{
    Scope scope{};
    Identity function{};
    Extent extent{};
    bool available{};
    Reason reason{Reason::Unavailable};
    bool persistent() const { return available && scope.persistent() && function.persistent(); }
};
struct Key
{
    std::uint64_t frame{}, node{}, context{};
    unsigned role{};
    bool operator==(const Key&) const = default;
};
struct Pair
{
    std::uint64_t id{}, parent{}, pre_ms{}, post_ms{}, lua_marks{}, marker{};
    unsigned depth{};
    Key key{};
    Snapshot before{}, after{};
    bool qualified{}, lua_inside{};
    Reason reason{Reason::Unavailable};
};
// All formal inputs must be by-value ObjectProperty params, not local/output/reference fields.
inline bool input_bounds(std::int32_t offset, std::int32_t size, unsigned parms_size,
                         bool object_property, bool parm, bool indirect)
{
    return object_property && parm && !indirect && size == sizeof(void*) && offset >= 0
        && parms_size > 0 && parms_size <= 65535 && static_cast<unsigned>(offset) <= parms_size
        && static_cast<unsigned>(size) <= parms_size - static_cast<unsigned>(offset);
}
class State
{
    struct Pending { Pair pair{}; };
    std::array<Pending,kMaxDepth> pending_{};
    std::array<Pair,kMaxPairs> completed_{};
    unsigned depth_{}, size_{}, limit_{};
    std::uint64_t next_id_{};
public:
    Scope scope{};
    bool active{};
    std::uint64_t deadline{}, entries{}, unmatched{}, discarded{}, marks_outside{};
    Reason last{Reason::Off};
    unsigned pending() const { return depth_; }
    unsigned pending_role() const { return depth_ ? pending_[depth_-1].pair.key.role : 0; }
    unsigned completed() const { return size_; }
    const Pair& at(unsigned i) const { return completed_[i]; }
    void drain() { size_ = 0; }
    bool begin(const Scope& s, std::uint64_t now, std::uint64_t duration, unsigned limit)
    {
        if (!s.present() || duration == 0 || duration > kMaxDurationMs || limit == 0
            || limit > kMaxPairs || now > UINT64_MAX-duration) return false;
        *this = {};
        scope=s; active=true; deadline=now+duration; limit_=limit; last=Reason::None;
        return true;
    }
    void stop(Reason why = Reason::Off)
    {
        discarded += depth_; depth_=0; active=false; last=why;
    }
    void poll(std::uint64_t now)
    {
        if (active && now >= deadline) stop(Reason::Expired);
    }
    bool accepts(std::uint64_t now)
    {
        poll(now);
        return active && entries < limit_;
    }
    bool guard_entry(const Key& key,std::uint64_t now)
    {
        poll(now);
        if (!active) return false;
        // Lua POST has no FFrame token. Never let recursive same-function POST
        // be mistaken for an ancestor, even when its entry exceeds the capture budget.
        for (unsigned i=0;i<depth_;++i)
            if (pending_[i].pair.key.role == key.role) { stop(Reason::Ordering); return false; }
        return entries < limit_;
    }
    void pre(const Key& key, const Snapshot& snapshot, std::uint64_t now)
    {
        if (!guard_entry(key,now)) return;
        if (!key.frame || !key.node || key.context != scope.pawn.address || key.role < 1 || key.role > 2)
        { last=Reason::Scope; return; }
        if (depth_ == kMaxDepth) { stop(Reason::Depth); return; }
        for (unsigned i=0;i<depth_;++i)
            if (pending_[i].pair.key == key) { stop(Reason::Ordering); return; }
        Pair p{};
        p.id=++next_id_; p.parent=depth_ ? pending_[depth_-1].pair.id : 0;
        p.depth=depth_; p.pre_ms=now; p.key=key; p.before=snapshot;
        pending_[depth_++].pair=p; ++entries;
    }
    // Lookup is deliberately pending-only; no stale last-call entry can be exposed/marked.
    bool mark(const Scope& fresh, unsigned role, std::uint64_t marker, std::uint64_t now)
    {
        poll(now);
        if (!active || !depth_ || !(fresh == scope) || pending_[depth_-1].pair.key.role != role
            || !pending_[depth_-1].pair.before.available)
        { ++marks_outside; return false; }
        auto& p=pending_[depth_-1].pair;
        if (p.lua_marks >= 4) { last=Reason::Budget; return false; }
        ++p.lua_marks; p.marker=marker; p.lua_inside=true; return true;
    }
    bool has(const Key& key) const
    {
        for (unsigned i=0;i<depth_;++i) if (pending_[i].pair.key == key) return true;
        return false;
    }
    void post(const Key& key, const Snapshot& snapshot, std::uint64_t now)
    {
        poll(now);
        if (!active) return;
        unsigned index=depth_;
        for (unsigned i=0;i<depth_;++i) if (pending_[i].pair.key == key) { index=i; break; }
        // Unobserved nested entries after the budget is exhausted never corrupt their outer pair.
        if (index == depth_)
        {
            ++unmatched;
            if (entries < limit_) stop(Reason::Ordering);
            else last=Reason::Budget;
            return;
        }
        if (index+1 != depth_) { stop(Reason::Ordering); return; }
        auto p=pending_[--depth_].pair;
        p.post_ms=now; p.after=snapshot;
        if (!p.before.available) p.reason=p.before.reason;
        else if (!p.after.available) p.reason=p.after.reason;
        else if (!(p.before.scope == scope) || !(p.after.scope == scope)
            || !(p.before.function == p.after.function)) p.reason=Reason::Identity;
        else if (!p.before.extent.valid() || !p.after.extent.valid()) p.reason=Reason::Extent;
        else if (!p.before.persistent() || !p.after.persistent()) p.reason=Reason::Lifetime;
        else if (!p.lua_inside) p.reason=Reason::NoLuaMark;
        else { p.reason=Reason::None; p.qualified=true; }
        if (size_ < kMaxPairs) completed_[size_++]=p;
        else { ++discarded; last=Reason::Budget; }
        if (entries == limit_ && depth_ == 0) { active=false; last=Reason::Budget; }
    }
};
}

