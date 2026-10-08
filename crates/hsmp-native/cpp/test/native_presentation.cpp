// Rejection/lifetime checks for the production provider; no mocked native parity claim.
#include "../src/native_presentation.cpp"
#include <iostream>
#include <limits>
#include <thread>
static const HsmpPresentation* installed{};
extern "C" void hsmp_native_set_presentation(const HsmpPresentation* p) {installed=p;}
namespace {
int checks{}, touches{};
void check(bool ok,const char* why) {++checks;if(!ok)throw std::runtime_error(why);}
template<class F> void rejects(F f,const char* why) {bool refused{};try{f();}catch(const Error&){refused=true;}check(refused,why);}
struct MockObject {uint64_t name;MockObject* cls;};
MockObject cls{10,nullptr}, obj{11,&cls};
uint64_t weak(void* p) {return p==&cls?2:p==&obj?1:0;}
void* resolve(uint64_t id) {++touches;return id==2?&cls:id==1?&obj:nullptr;}
void* class_of(void* p) {++touches;return p==&cls?&cls:static_cast<MockObject*>(p)->cls;}
const uint64_t* mock_name(const void* p) {++touches;return &static_cast<const MockObject*>(p)->name;}
int32_t guard_check(void* context) {return *static_cast<int32_t*>(context);}
}
int main() {
    try {
        hsmp_presentation_register(nullptr);check(installed==nullptr,"null provider registration");
        HsmpViewResult result{};check(inspect({}, {}, {}, nullptr, &result)==-1 && result.complete==0,"missing reflection refused");
        Transform pose{{1.25,-2.5,7},{0.1,0.2,0.3,0.9},{0.75,1.5,2}};
        auto native=engine(pose);check(native.p[0]==pose.p[0]&&native.q[0]==pose.q[0]&&native.scale[2]==pose.scale[2],"native transform field ordering");
        check(close(wire(native),pose),"native transform roundtrip");
        pose.q[0]=std::numeric_limits<double>::quiet_NaN();rejects([&]{engine(pose);},"NaN quaternion refused");
        HsmpViewComponent c{};c.relative={{0,0,0},{0,0,0,1},{1,1,1}};c.visible=1;
        c.kind=2;rejects([&]{recipe(c);},"groom cannot become ready");c.kind=0;
        c.vertex_state=3;rejects([&]{recipe(c);},"unavailable vertex state refused");c.vertex_state=1;
        rejects([&]{recipe(c);},"captured colors require actual LOD data");c.vertex_state=0;
        c.bone_count=513;rejects([&]{recipe(c);},"oversized bone dictionary refused");c.bone_count=0;
        HsmpViewFrame f{};f.bone_count=1;rejects([&]{frame(c,f);},"source frame dictionary mismatch refused");
        HsmpReflect reflect{};reflect.abi=HSMP_REFLECT_ABI;reflect.weak=weak;reflect.resolve=resolve;reflect.class_of=class_of;
        vt=&reflect;object_name=mock_name;game_thread=0;identities.clear();thread();
        auto object=keep(&obj);check(get(object)==&obj,"live weak identity");
        int32_t valid=1;HsmpViewGuard guard{&valid,guard_check};active_guard=&guard;
        check(get(object)==&obj,"borrowed guard accepts current binding");
        valid=0;touches=0;rejects([&]{get(object);},"changed binding refused between calls");
        check(touches==0,"guard refusal precedes all native object access");active_guard=nullptr;
        touches=0;check(inspect({}, {}, {}, &guard, &result)==-1&&touches==0,"provider entry refuses changed guard");
        check(active_guard==nullptr,"failed guard does not escape the borrowed call");
        obj.name=12;rejects([&]{get(object);},"serial-zero name reuse refused");
        rejects([&]{keep(&obj);},"reenrollment cannot overwrite old identity");obj.name=11;
        int32_t status{};touches=0;std::thread other([&]{HsmpViewResult out{};status=inspect({}, {}, {}, nullptr, &out);});other.join();
        check(status==-1&&touches==0,"wrong thread refuses before touching engine objects");
        mirrors.emplace(77,Mirror{{1,1},{2,2},{}});touches=0;discard(77);
        check(mirrors.empty()&&touches==0,"world discard never touches old UObject");
        mirrors.emplace(78,Mirror{{1,1},{2,2},{}});touches=0;destroy({3,3},78);
        check(mirrors.empty()&&touches==0,"wrong-world destroy refuses before old actor access");
        std::cout<<checks<<" native presentation lifetime/rejection checks passed\n";return 0;
    }catch(const std::exception& e){std::cerr<<e.what()<<'\n';return 1;}
}
