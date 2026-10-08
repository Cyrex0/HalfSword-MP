// Fixed, scalar-only enrollment diagnostics. No clock or engine calls here.
#pragma once
#include <array>
#include <cstdint>
namespace hsmp_box
{
struct StageCost
{
    const char* name{};
    std::uint64_t start_ms{}, end_ms{};
    bool available{};
};
class EnrollmentTiming
{
    bool active_{};
public:
    static constexpr unsigned kLimit=16;
    std::array<StageCost,kLimit> rows{};
    unsigned count{};
    bool overflow{};
    bool active() const { return active_; }
    void begin() { *this={}; active_=true; }
    void stage(const char* name,std::uint64_t now)
    {
        if (!active_) return;
        if (count) { auto& r=rows[count-1]; r.end_ms=now; r.available=now>=r.start_ms; }
        if (count==kLimit) { overflow=true; active_=false; return; }
        rows[count++]={name,now,now,false};
    }
    void finish(std::uint64_t now)
    {
        if (!active_) return;
        if (count) { auto& r=rows[count-1]; r.end_ms=now; r.available=now>=r.start_ms; }
        active_=false;
    }
};
}
