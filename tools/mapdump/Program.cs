// HSMP MapDump: static extraction of spawn points / world objects / runtime spawners from
// Half Sword cooked maps, reading the game pak directly (read-only) via CUE4Parse + usmap.
// Usage: dotnet MapDump.dll [--out <dir>] [--all] [map ...] | --probe <pkg> [export|*] | --scan <pathRx> <n1,n2> | --selftest
using System.Text.RegularExpressions;
using CUE4Parse.Compression;
using CUE4Parse.Encryption.Aes;
using CUE4Parse.FileProvider;
using CUE4Parse.MappingsProvider.Usmap;
using CUE4Parse.UE4.Assets;
using CUE4Parse.UE4.Assets.Exports;
using CUE4Parse.UE4.Objects.Core.Math;
using CUE4Parse.UE4.Objects.Core.Misc;
using CUE4Parse.UE4.Objects.UObject;
using CUE4Parse.UE4.Versions;
using Newtonsoft.Json;
using Newtonsoft.Json.Linq;

namespace MapDump;

public static class Program
{
    // Paths come from the environment (run.ps1 sets them): HSMP_GAME_DIR = the game install
    // (contains HalfswordUE5\), HSMP_OODLE_DLL = your own copy of oo2core_9_win64.dll (never
    // committed), HSMP_PAK_AES = the pak AES key (the Half Sword developers share it with
    // modders; the default below is that key).
    static string GameRoot => Path.Combine(Env("HSMP_GAME_DIR", Path.Combine(RepoRoot, "game")), "HalfswordUE5");
    static string AesKey => Env("HSMP_PAK_AES", "0xbcbf7b45a4a8150d06f7b955bc25ef5ce603470f508302cad0eb48fea2d91517");
    static string OodleDll => Env("HSMP_OODLE_DLL", Path.Combine(RepoRoot, "workspace", "tools", "repak", "oo2core_9_win64.dll"));
    static string RepoRoot => Env("HSMP_ROOT", Directory.GetCurrentDirectory());
    static string Env(string name, string fallback) =>
        Environment.GetEnvironmentVariable(name) is { Length: > 0 } v ? v : fallback;

    public static readonly string[] Arenas =
    {
        "Map_Arena_Alley", "Map_Arena_Pit", "Map_Arena_Yard", "Map_Arena_Slums",
        "Map_Arena_Cellar", "Map_Arena_LordsHall", "Map_Arena_EastTower",
    };

    public static DefaultFileProvider Provider = null!;

    public static int Main(string[] args)
    {
        if (args.FirstOrDefault() == "--inventory-selftest") return InventoryHarvester.SelfTest();
        if (args.FirstOrDefault() == "--inventory-catalogue")
        {
            if (args.Length != 2) { Console.Error.WriteLine("Usage: --inventory-catalogue directory"); return 1; }
            return InventoryHarvester.RefreshCatalogue(Path.GetFullPath(args[1]));
        }
        if (args.FirstOrDefault() == "--inventory")
        {
            var inventoryOut = Path.Combine(RepoRoot, "test-results", "dev-feature-checks", "inventory-harvest");
            if (args.Length == 3 && args[1] == "--out") inventoryOut = Path.GetFullPath(args[2]);
            else if (args.Length != 1) { Console.Error.WriteLine("Usage: --inventory [--out directory]"); return 1; }
            if (!InventoryHarvester.InitializeQuietly(Init)) return 1;
            return InventoryHarvester.Run(Provider, inventoryOut);
        }
        string outDir = Path.Combine(RepoRoot, "docs", "arena_static");
        bool all = false;
        var maps = new List<string>();
        for (int i = 0; i < args.Length; i++)
        {
            switch (args[i])
            {
                case "--out": outDir = args[++i]; break;
                case "--all": all = true; break;
                case "--scan":
                    Init();
                    Scan(args[++i], args[++i].Split(','));
                    return 0;
                case "--selftest":
                    SelfTest();
                    return 0;
                case "--probe":
                    Init();
                    Provider.ReadScriptData = true;
                    Probe(args[++i], i + 1 < args.Length ? args[++i] : null);
                    return 0;
                default: maps.Add(args[i]); break;
            }
        }
        Init();
        Directory.CreateDirectory(outDir);

        var targets = new List<string>();
        if (maps.Count == 0) maps.AddRange(Arenas);
        foreach (var m in maps)
        {
            var p = ResolveMap(m);
            if (p == null) { Console.Error.WriteLine($"!! map not found: {m}"); continue; }
            targets.Add(p);
        }
        if (all)
        {
            foreach (var f in Provider.Files.Keys.Where(k => k.EndsWith(".umap", StringComparison.OrdinalIgnoreCase)).OrderBy(k => k))
                if (!targets.Contains(f, StringComparer.OrdinalIgnoreCase)) targets.Add(f);
        }

        // combat scenario definitions (which map, combatant count, modes, tiers) - drive spawn-point selection
        var ce = new JObject();
        foreach (var f in Provider.Files.Keys.Where(k => k.Contains("/DataAssets/CombatScenarios/") && k.EndsWith(".uasset")).OrderBy(k => k))
        {
            try
            {
                var pkg = Provider.LoadPackage(f);
                var cdo = pkg.GetExports().FirstOrDefault(e => e.Name.StartsWith("Default__"));
                if (cdo != null) ce[Path.GetFileNameWithoutExtension(f)] = new MapExtractor(Provider).EffectivePropsPublic(cdo);
            }
            catch (Exception ex) { ce[Path.GetFileNameWithoutExtension(f)] = "error: " + ex.Message; }
        }
        File.WriteAllText(Path.Combine(outDir, "_combat_events.json"), ce.ToString(Formatting.Indented));

        var index = new JArray();
        foreach (var t in targets)
        {
            var name = Path.GetFileNameWithoutExtension(t);
            var isArena = Arenas.Contains(name);
            var file = isArena ? Path.Combine(outDir, name + ".json") : Path.Combine(outDir, "all_maps", SafeName(t) + ".json");
            Directory.CreateDirectory(Path.GetDirectoryName(file)!);
            var sw = System.Diagnostics.Stopwatch.StartNew();
            JObject result;
            try
            {
                var d = new MapExtractor(Provider);
                result = d.Run(t);
            }
            catch (Exception ex)
            {
                result = new JObject { ["map"] = name, ["package"] = t, ["fatal_error"] = ex.ToString() };
            }
            File.WriteAllText(file, result.ToString(Formatting.Indented));
            var summary = Summarize(result, t, file, outDir);
            index.Add(summary);
            Console.WriteLine($"{name}: {summary["spawn_points"]} spawns, {summary["world_objects"]} objects, {summary["spawners"]} spawners, errors={summary["errors"]} ({sw.ElapsedMilliseconds} ms)");
        }
        if (all)
        {
            // which top-level maps pull each package in, and which non-map assets reference each map by name
            var refs = MapNameReferences(targets);
            foreach (JObject e in index)
            {
                var pkg = (string)e["package"]!;
                var parents = new JArray();
                foreach (JObject o in index)
                    if (o != e && o["_sources"] is JArray srcs && srcs.Any(x => string.Equals((string?)x, pkg, StringComparison.OrdinalIgnoreCase)))
                        parents.Add(o["map"]);
                e["pulled_in_by"] = parents;
                if (parents.Count > 0 && (string?)e["kind"] != "level_instance") e["kind"] = "sublevel";
                e["referenced_by_assets"] = refs.TryGetValue(Path.GetFileNameWithoutExtension(pkg), out var r) ? new JArray(r) : new JArray();
            }
        }
        foreach (JObject e in index) e.Remove("_sources");
        File.WriteAllText(Path.Combine(outDir, all ? "_index.json" : "_index_arenas.json"), index.ToString(Formatting.Indented));
        return 0;
    }

    static string SafeName(string pkgPath)
    {
        var s = pkgPath.Replace("HalfswordUE5/Content/", "").Replace(".umap", "");
        return Regex.Replace(s, @"[\\/]", "__");
    }

    static JObject Summarize(JObject r, string pkg, string file, string outDir)
    {
        var cats = new JObject();
        if (r["world_objects"] is JArray wo)
            foreach (var g in wo.GroupBy(o => (string?)o["category"] ?? "?").OrderBy(g => g.Key))
                cats[g.Key] = g.Count();
        return new JObject
        {
            ["map"] = Path.GetFileNameWithoutExtension(pkg),
            ["package"] = pkg,
            ["json"] = Path.GetRelativePath(outDir, file).Replace('\\', '/'),
            ["kind"] = r["kind"],
            ["sources"] = (r["sources"] as JArray)?.Count ?? 0,
            ["spawn_points"] = (r["spawn_points"] as JArray)?.Count ?? 0,
            ["world_objects"] = (r["world_objects"] as JArray)?.Count ?? 0,
            ["world_object_categories"] = cats,
            ["spawners"] = (r["spawners"] as JArray)?.Count ?? 0,
            ["other_actors"] = (r["other_actors"] as JArray)?.Count ?? 0,
            ["errors"] = (r["errors"] as JArray)?.Count ?? (r["fatal_error"] != null ? 1 : 0),
            ["parse_status"] = r["fatal_error"] != null ? "fatal" : ((r["errors"] as JArray)?.Count > 0 ? "partial" : "ok"),
            ["spawn_point_list"] = new JArray((r["spawn_points"] as JArray ?? new JArray()).Select(sp => new JObject
            {
                ["name"] = sp["name"], ["location"] = sp["location"], ["yaw"] = sp["rotation"]?["yaw"],
            })),
            ["_sources"] = new JArray((r["sources"] as JArray ?? new JArray()).Skip(1).Select(x => x["file"])),
        };
    }

    public static string? ResolveMap(string m)
    {
        if (Provider.Files.ContainsKey(m)) return m;
        var n = m.EndsWith(".umap") ? m : m + ".umap";
        return Provider.Files.Keys.FirstOrDefault(k => k.EndsWith("/" + n, StringComparison.OrdinalIgnoreCase));
    }

    static void Init()
    {
        OodleHelper.Initialize(OodleDll);
        // Only the top-level Paks dir: LogicMods (UE4SS mod paks) are deliberately excluded.
        Provider = new DefaultFileProvider(Path.Combine(GameRoot, @"Content\Paks"), SearchOption.TopDirectoryOnly,
            new VersionContainer(EGame.GAME_UE5_4), StringComparer.OrdinalIgnoreCase);
        var usmap = Directory.GetFiles(Path.Combine(GameRoot, @"Binaries\Win64\ue4ss"), "*.usmap").First();
        Provider.MappingsContainer = new FileUsmapTypeMappingsProvider(usmap);
        Provider.Initialize();
        Provider.SubmitKey(new FGuid(), new FAesKey(AesKey));
        Provider.PostMount();
    }

    // --scan <pathRegex> <needle1,needle2>: list packages whose name map contains all needles
    static void Scan(string pathRx, string[] needles)
    {
        var rx = new Regex(pathRx, RegexOptions.IgnoreCase);
        foreach (var f in Provider.Files.Keys.Where(k => (k.EndsWith(".uasset") || k.EndsWith(".umap")) && rx.IsMatch(k)).OrderBy(k => k))
        {
            try
            {
                var pkg = Provider.LoadPackage(f) as AbstractUePackage;
                if (pkg == null) continue;
                var names = pkg.NameMap.Select(n => n.Name).ToHashSet();
                if (needles.All(nd => names.Any(n => n != null && n.Contains(nd)))) Console.WriteLine(f);
            }
            catch (Exception ex) { Console.Error.WriteLine($"{f}: {ex.Message}"); }
        }
    }

    // For every map short name, list non-umap packages whose name table mentions it (OpenLevel targets, map lists in GI/UI)
    static Dictionary<string, List<string>> MapNameReferences(List<string> maps)
    {
        var names = maps.Select(m => Path.GetFileNameWithoutExtension(m)).Distinct().ToList();
        var res = names.ToDictionary(n => n, _ => new List<string>(), StringComparer.OrdinalIgnoreCase);
        foreach (var f in Provider.Files.Keys.Where(k => k.EndsWith(".uasset") && k.StartsWith("HalfswordUE5/Content/")))
        {
            try
            {
                if (Provider.LoadPackage(f) is not AbstractUePackage pkg) continue;
                var nm = pkg.NameMap.Select(n => n.Name ?? "").ToList();
                foreach (var n in names)
                    if (nm.Any(x => x.Equals(n, StringComparison.OrdinalIgnoreCase) || x.EndsWith("/" + n, StringComparison.OrdinalIgnoreCase) || x.EndsWith("/" + n + "." + n, StringComparison.OrdinalIgnoreCase)))
                        res[n].Add(f.Replace("HalfswordUE5/Content/", "/Game/").Replace(".uasset", ""));
            }
            catch { }
        }
        return res;
    }

    static void SelfTest()
    {
        // child at (100,0,0) yaw 30 inside parent at (10,20,30) yaw 90 scale 2 -> (10,220,30) yaw 120
        var parent = new Xf { T = new V3(10, 20, 30), R = Quat.FromRotator(0, 90, 0), S = new V3(2, 2, 2) };
        var child = new Xf { T = new V3(100, 0, 0), R = Quat.FromRotator(0, 30, 0), S = V3.One };
        Console.WriteLine(Xf.Compose(child, parent).ToJson().ToString(Formatting.None));
        // pitch/roll round trip
        var q = Quat.FromRotator(20, -60, 10); Console.WriteLine(q.ToRotator());
        // parent pitch 90: child +X goes to +Z
        var p2 = new Xf { T = new V3(0, 0, 0), R = Quat.FromRotator(90, 0, 0), S = V3.One };
        Console.WriteLine(Xf.Compose(new Xf { T = new V3(100, 0, 0), R = Quat.Identity, S = V3.One }, p2).ToJson().ToString(Formatting.None));
    }

    static void Probe(string pkgPath, string? exportName)
    {
        var p = ResolveMap(pkgPath) ?? pkgPath;
        var pkg = Provider.LoadPackage(p);
        foreach (var e in pkg.GetExports())
        {
            if (exportName == null)
                Console.WriteLine($"{e.Name}\t{e.ExportType}\t{e.Class?.GetPathName()}\t{e.Outer?.Name}");
            else if (e.Name == exportName || e.Outer?.Name == exportName || exportName == "*")
                Console.WriteLine(JsonConvert.SerializeObject(e, Formatting.Indented));
        }
    }
}

// ---------------------------------------------------------------- transforms
public struct Quat
{
    public double X, Y, Z, W;
    public Quat(double x, double y, double z, double w) { X = x; Y = y; Z = z; W = w; }
    public static readonly Quat Identity = new(0, 0, 0, 1);

    public static Quat FromRotator(double pitch, double yaw, double roll)
    {
        const double h = Math.PI / 360.0;
        double sp = Math.Sin(pitch * h), cp = Math.Cos(pitch * h);
        double sy = Math.Sin(yaw * h), cy = Math.Cos(yaw * h);
        double sr = Math.Sin(roll * h), cr = Math.Cos(roll * h);
        return new Quat(cr * sp * sy - sr * cp * cy, -cr * sp * cy - sr * cp * sy, cr * cp * sy - sr * sp * cy, cr * cp * cy + sr * sp * sy);
    }

    public (double pitch, double yaw, double roll) ToRotator()
    {
        double st = Z * X - W * Y;
        double yawY = 2 * (W * Z + X * Y), yawX = 1 - 2 * (Y * Y + Z * Z);
        const double th = 0.4999995, r2d = 180.0 / Math.PI;
        double pitch, yaw, roll;
        if (st < -th) { pitch = -90; yaw = Math.Atan2(yawY, yawX) * r2d; roll = Norm(-yaw - 2 * Math.Atan2(X, W) * r2d); }
        else if (st > th) { pitch = 90; yaw = Math.Atan2(yawY, yawX) * r2d; roll = Norm(yaw - 2 * Math.Atan2(X, W) * r2d); }
        else
        {
            pitch = Math.Asin(2 * st) * r2d; yaw = Math.Atan2(yawY, yawX) * r2d;
            roll = Math.Atan2(-2 * (W * X + Y * Z), 1 - 2 * (X * X + Y * Y)) * r2d;
        }
        return (pitch, yaw, roll);
    }

    static double Norm(double a) { a %= 360; if (a > 180) a -= 360; if (a < -180) a += 360; return a; }

    public static Quat operator *(Quat a, Quat b) => new(
        a.W * b.X + a.X * b.W + a.Y * b.Z - a.Z * b.Y,
        a.W * b.Y - a.X * b.Z + a.Y * b.W + a.Z * b.X,
        a.W * b.Z + a.X * b.Y - a.Y * b.X + a.Z * b.W,
        a.W * b.W - a.X * b.X - a.Y * b.Y - a.Z * b.Z);

    public V3 Rotate(V3 v)
    {
        var q = new V3(X, Y, Z);
        var t = V3.Cross(q, v) * 2;
        return v + t * W + V3.Cross(q, t);
    }
}

public struct V3
{
    public double X, Y, Z;
    public V3(double x, double y, double z) { X = x; Y = y; Z = z; }
    public static V3 operator +(V3 a, V3 b) => new(a.X + b.X, a.Y + b.Y, a.Z + b.Z);
    public static V3 operator *(V3 a, double s) => new(a.X * s, a.Y * s, a.Z * s);
    public static V3 Mul(V3 a, V3 b) => new(a.X * b.X, a.Y * b.Y, a.Z * b.Z);
    public static V3 Cross(V3 a, V3 b) => new(a.Y * b.Z - a.Z * b.Y, a.Z * b.X - a.X * b.Z, a.X * b.Y - a.Y * b.X);
    public static readonly V3 One = new(1, 1, 1);
}

public struct Xf
{
    public V3 T; public Quat R; public V3 S;
    public static readonly Xf Identity = new() { T = new V3(0, 0, 0), R = Quat.Identity, S = V3.One };
    // child (local) expressed in parent space -> world
    public static Xf Compose(Xf child, Xf parent, bool absLoc = false, bool absRot = false, bool absScale = false) => new()
    {
        T = absLoc ? child.T : parent.R.Rotate(V3.Mul(parent.S, child.T)) + parent.T,
        R = absRot ? child.R : parent.R * child.R,
        S = absScale ? child.S : V3.Mul(parent.S, child.S),
    };

    public JObject ToJson()
    {
        var (p, y, r) = R.ToRotator();
        return new JObject
        {
            ["location"] = new JArray(Rd(T.X), Rd(T.Y), Rd(T.Z)),
            ["rotation"] = new JObject { ["pitch"] = Rd(p), ["yaw"] = Rd(y), ["roll"] = Rd(r) },
            ["scale"] = new JArray(Rd(S.X), Rd(S.Y), Rd(S.Z)),
        };
    }
    static double Rd(double v) { var x = Math.Round(v, 3); return x == 0 ? 0 : x; }
}

// ---------------------------------------------------------------- extraction
public class MapExtractor
{
    readonly DefaultFileProvider _p;
    readonly JArray _sources = new(), _spawns = new(), _objects = new(), _spawners = new(), _other = new(), _errors = new(), _warnings = new();
    int _staticSma;
    readonly HashSet<string> _visiting = new(StringComparer.OrdinalIgnoreCase);
    static readonly Dictionary<string, List<string>> ClassChainCache = new();
    static readonly Dictionary<string, Dictionary<string, string>> EnumCache = new();

    public MapExtractor(DefaultFileProvider p) { _p = p; }

    public JObject Run(string pkgPath)
    {
        ProcessLevel(pkgPath, Xf.Identity, new List<string>(), "persistent");
        string kind = DetectKind(pkgPath);
        return new JObject
        {
            ["map"] = Path.GetFileNameWithoutExtension(pkgPath),
            ["package"] = pkgPath,
            ["kind"] = kind,
            ["units"] = "UE units: cm, degrees (pitch/yaw/roll), world space of the root map",
            ["sources"] = _sources,
            ["spawn_points"] = _spawns,
            ["world_objects"] = _objects,
            ["spawners"] = _spawners,
            ["other_actors"] = _other,
            ["static_mesh_actors_static_count"] = _staticSma,
            ["errors"] = _errors,
            ["warnings"] = _warnings,
        };
    }

    string DetectKind(string pkgPath)
    {
        // persistent if it has a GameMode override / streams levels / is under Maps root with a LevelScript
        int streamed = _sources.Count(s => (string?)s["via"] != "persistent");
        var name = Path.GetFileNameWithoutExtension(pkgPath);
        if (pkgPath.Contains("/LevelInstances/", StringComparison.OrdinalIgnoreCase) || name.StartsWith("LI_")) return "level_instance";
        if (Regex.IsMatch(name, @"_(Lighting|Details|Geometry|PhysicsProps|Props|Foliage|Rocks|Terrain|VFX|Architecture|Audio|Spikes|NarrowPassage_Spikes)$", RegexOptions.IgnoreCase))
            return "sublevel";
        return streamed > 0 ? "persistent_with_sublevels" : "persistent_or_standalone";
    }

    // ------------------------------------------------------------ levels
    void ProcessLevel(string pkgPath, Xf levelXf, List<string> chain, string via)
    {
        if (!_visiting.Add(pkgPath + "|" + string.Join(">", chain)) || chain.Count > 8) return;
        IPackage pkg;
        try { pkg = _p.LoadPackage(pkgPath); }
        catch (Exception ex) { _errors.Add($"load {pkgPath}: {ex.Message}"); return; }

        var myChain = new List<string>(chain) { pkgPath };
        var src = new JObject { ["file"] = pkgPath, ["via"] = via, ["parent_chain"] = new JArray(chain), ["level_transform"] = levelXf.ToJson() };
        _sources.Add(src);

        UObject[] exports;
        try { exports = pkg.GetExports().ToArray(); }
        catch (Exception ex) { _errors.Add($"exports {pkgPath}: {ex.Message}"); return; }

        // streaming sublevels
        foreach (var ls in exports.Where(e => e.ExportType.StartsWith("LevelStreaming")))
        {
            try
            {
                var world = Prop<FSoftObjectPath>(ls, "WorldAsset");
                var asset = world.AssetPathName.Text;
                if (string.IsNullOrEmpty(asset) || asset == "None")
                {
                    // LevelStreamingDynamic may store PackageNameToLoad instead
                    asset = Prop<FName>(ls, "PackageNameToLoad").Text;
                }
                var sub = GamePathToPkg(asset);
                var sxf = Xf.Identity;
                if (TryProp<FTransform>(ls, "LevelTransform", out var lt))
                {
                    sxf = new Xf { T = new V3(lt.Translation.X, lt.Translation.Y, lt.Translation.Z), R = new Quat(lt.Rotation.X, lt.Rotation.Y, lt.Rotation.Z, lt.Rotation.W), S = new V3(lt.Scale3D.X, lt.Scale3D.Y, lt.Scale3D.Z) };
                    if (sxf.S.X == 0 && sxf.S.Y == 0 && sxf.S.Z == 0) sxf.S = V3.One;
                }
                var flags = $"{ls.ExportType}" + (TryProp<bool>(ls, "bInitiallyLoaded", out var il) ? $" initiallyLoaded={il}" : "") + (TryProp<bool>(ls, "bInitiallyVisible", out var iv) ? $" initiallyVisible={iv}" : "");
                if (sub == null) { _errors.Add($"streaming level {ls.Name} in {pkgPath}: unresolved '{asset}'"); continue; }
                ProcessLevel(sub, Xf.Compose(sxf, levelXf), myChain, $"streaming:{ls.Name} ({flags})");
            }
            catch (Exception ex) { _errors.Add($"streaming {ls.Name} in {pkgPath}: {ex.Message}"); }
        }

        // actors = exports whose outer is a Level
        var actors = exports.Where(e => e.Outer != null && e.Outer.Name.Text == "PersistentLevel").ToList();
        foreach (var a in actors)
        {
            try { HandleActor(a, exports, levelXf, pkgPath, myChain, via); }
            catch (Exception ex) { _errors.Add($"actor {a.Name} in {pkgPath}: {ex.GetType().Name}: {ex.Message}"); }
        }
    }

    string? GamePathToPkg(string? asset)
    {
        if (string.IsNullOrEmpty(asset) || asset == "None") return null;
        var a = asset;
        var dot = a.LastIndexOf('.');
        if (dot > a.LastIndexOf('/')) a = a[..dot];
        if (a.StartsWith("/Game/")) a = "HalfswordUE5/Content/" + a[6..];
        else if (a.StartsWith("/")) a = a[1..];
        var p = a + ".umap";
        return _p.Files.ContainsKey(p) ? p : null;
    }

    // ------------------------------------------------------------ actors
    static readonly Regex SpawnRx = new(@"Spawn(er)?Point|PlayerStart|TargetPoint|SpawnLocation|Spawner_?Point", RegexOptions.IgnoreCase);
    static readonly Regex SpawnerRx = new(@"Spawner|Manager|Rack|Container|Chest|GameMode|LevelScript|Arsenal|Armory|Stand|Loot|Shop|Gamemode|Director|Wave", RegexOptions.IgnoreCase);

    static readonly (Regex rx, string cat)[] CatRules =
    {
        (new(@"^BP_Barrel_Destructable|Destructible|Destructable", RegexOptions.IgnoreCase), "destructible"),
        (new(@"^BP_Fence_(Flimsy|Bags)", RegexOptions.IgnoreCase), "fence"),
        (new(@"^(Trap_Kettle_BP|Trap_BP|BP_Structure_Trap|BP_Weapon_Trap|.*Trap)", RegexOptions.IgnoreCase), "trap"),
        (new(@"^Chain_BP", RegexOptions.IgnoreCase), "chain"),
        (new(@"^ST_Lever", RegexOptions.IgnoreCase), "lever"),
        (new(@"Chandelier|Candle", RegexOptions.IgnoreCase), "light_prop"),
        (new(@"^BP_Container_|Chest", RegexOptions.IgnoreCase), "container"),
        (new(@"Training_Dummy|Dummy", RegexOptions.IgnoreCase), "training_dummy"),
        (new(@"Quiver", RegexOptions.IgnoreCase), "quiver"),
    };

    void HandleActor(UObject a, UObject[] exports, Xf levelXf, string pkgPath, List<string> chain, string via)
    {
        var cls = a.Class;
        var className = cls?.Name.Text ?? a.ExportType;
        var classPath = cls?.GetPathName() ?? a.ExportType;
        var classChain = ClassChain(a);

        if (className is "WorldSettings" or "Model" or "Brush" or "BrushComponent") return;

        var xf = ActorWorldXf(a, levelXf, out var attachInfo);

        // level instance -> recurse
        if (classChain.Contains("LevelInstance") || classChain.Contains("PackedLevelActor") && false)
        {
            var wa = Prop<FSoftObjectPath>(a, "CookedWorldAsset");
            if (string.IsNullOrEmpty(wa.AssetPathName.Text) || wa.AssetPathName.Text == "None") wa = Prop<FSoftObjectPath>(a, "WorldAsset");
            var sub = GamePathToPkg(wa.AssetPathName.Text);
            _other.Add(Brief(a, className, xf, pkgPath, $"level_instance -> {wa.AssetPathName.Text}"));
            if (sub == null)
            {
                if (string.IsNullOrEmpty(wa.AssetPathName.Text) || wa.AssetPathName.Text == "None")
                    _warnings.Add($"empty LevelInstance {a.Name} (label '{ActorLabel(a)}') in {pkgPath}: no CookedWorldAsset (nothing to load at runtime)");
                else _errors.Add($"level instance {a.Name} in {pkgPath}: unresolved '{wa.AssetPathName.Text}'");
                return;
            }
            ProcessLevel(sub, xf ?? levelXf, chain, $"level_instance:{a.Name} ({ActorLabel(a)})");
            return;
        }

        var comps = exports.Where(e => IsOuter(e.Outer, a) || (e.Outer != null && IsOuter(e.Outer.Outer, a))).ToList();
        var instProps = PropsJson(a, skipComponentRefs: true);
        var label = ActorLabel(a);

        var o = new JObject
        {
            ["name"] = a.Name,
            ["class"] = className,
            ["class_path"] = ToGamePath(classPath),
            ["class_chain"] = new JArray(classChain),
            ["source"] = pkgPath,
        };
        if (via != "persistent") o["via"] = via;
        var tags = EffectiveProps(a)["Tags"];
        if (tags is JArray ta && ta.Count > 0) o["tags"] = ta;
        if (xf != null) foreach (var kv in xf.Value.ToJson()) o[kv.Key] = kv.Value;
        else o["transform_note"] = "no RootComponent";
        if (attachInfo != null) o["attached_to"] = attachInfo;

        bool isWeapon = classChain.Any(c => c.StartsWith("ModularWeaponBP")) || classPath.Contains("/Weapons/Blueprints/");
        bool isSpawn = SpawnRx.IsMatch(className) || classChain.Any(c => c is "PlayerStart" or "TargetPoint");
        bool isSma = classChain.Contains("StaticMeshActor") || className == "StaticMeshActor";

        // physics
        var phys = new JArray();
        string? mobility = null; string? mesh = null;
        foreach (var c in comps)
        {
            if (!c.ExportType.EndsWith("Component") && !ClassChain(c).Any(x => x.EndsWith("Component"))) continue;
            var sim = ChainLookup(c, "BodyInstance", out var bi) && bi is JObject bj && bj["bSimulatePhysics"]?.Value<bool>() == true;
            if (!sim)
            {
                // BodyInstance is delta-serialized: walk templates for bSimulatePhysics specifically
                sim = StructFieldLookup(c, "BodyInstance", "bSimulatePhysics") == true;
            }
            var mob = ChainLookup(c, "Mobility", out var mv) ? mv?.ToString() : null;
            if (c == RootComp(a)) mobility = mob;
            if (sim) phys.Add(new JObject { ["component"] = c.Name, ["type"] = c.ExportType, ["mobility"] = mob, ["mesh"] = MeshOf(c) });
            if (isSma && c.ExportType == "StaticMeshComponent") mesh ??= MeshOf(c);
        }
        bool simulate = phys.Count > 0;

        string? cat = null;
        bool isTrapClass = Regex.IsMatch(className, "Trap", RegexOptions.IgnoreCase);
        if (isSpawn) cat = "spawn";
        else if (isTrapClass) cat = "trap";
        else if (isWeapon) cat = className.Contains("Quiver") ? "quiver" : "weapon";
        else
        {
            foreach (var (rx, c) in CatRules) if (rx.IsMatch(className)) { cat = c; break; }
            if (cat == null && isSma && (simulate || (mobility ?? "").Contains("Movable"))) cat = "static_mesh_actor_movable";
            if (cat == null && classChain.Contains("Character")) cat = "character";
            if (cat == null && simulate) cat = "physics_other";
        }

        bool isRuntimeSpawner = !Regex.IsMatch(className, "Lighting|Scalability|CloudMask|LightDetection") &&
            (SpawnerRx.IsMatch(className) || classChain.Contains("LevelScriptActor") ||
             (!isWeapon && !classChain.Contains("Character") && HasClassRefs(a)));

        if (isSpawn)
        {
            o["label"] = label;
            o["instance_properties"] = instProps;
            o["effective_properties"] = Slim(EffectiveProps(a));
            _spawns.Add(o);
        }
        else if (cat != null)
        {
            o["category"] = cat;
            if (label != null) o["label"] = label;
            o["simulate_physics"] = simulate;
            if (phys.Count > 0) o["simulating_components"] = phys;
            if (mobility != null) o["root_mobility"] = mobility;
            if (isSma) { o["static_mesh"] = mesh; }
            o["instance_properties"] = instProps;
            if (isWeapon)
            {
                o["weapon_overrides"] = WeaponOverrides(a);
                var eff = EffectiveProps(a);
                if (eff["Simulates Physics"] != null) o["bp_simulates_physics"] = eff["Simulates Physics"];
            }
            var cas = ChildActorInfo(comps);
            if (cas.Count > 0) o["child_actor_components"] = cas;
            _objects.Add(o);
        }

        if (isRuntimeSpawner && !isSpawn)
        {
            var s = new JObject
            {
                ["name"] = a.Name, ["class"] = className, ["class_path"] = ToGamePath(classPath), ["class_chain"] = new JArray(classChain), ["source"] = pkgPath,
            };
            if (xf != null) s["location"] = xf.Value.ToJson()["location"];
            s["instance_properties"] = instProps;
            s["effective_properties"] = Slim(EffectiveProps(a));
            var cas = ChildActorInfo(comps);
            if (cas.Count > 0) s["child_actor_components"] = cas;
            _spawners.Add(s);
        }

        if (!isSpawn && cat == null && !isRuntimeSpawner && isSma)
        {
            _staticSma++;
            return;
        }
        if (!isSpawn && cat == null && !isRuntimeSpawner)
        {
            var b = Brief(a, className, xf, pkgPath, null);
            if (isSma) b["static_mesh"] = mesh;
            if (o["tags"] != null) b["tags"] = o["tags"];
            _other.Add(b);
        }
    }

    static readonly Dictionary<UObject, string?> LabelCache = new(ReferenceEqualityComparer.Instance);
    static string? ActorLabel(UObject a)
    {
        if (LabelCache.TryGetValue(a, out var l)) return l;
        try { l = (string?)JObject.FromObject(a, JsonSerializer.Create(new JsonSerializerSettings { ReferenceLoopHandling = ReferenceLoopHandling.Ignore }))["ActorLabel"]; } catch { l = null; }
        LabelCache[a] = l; return l;
    }

    static bool IsOuter(ResolvedObject? outer, UObject a) =>
        outer != null && outer.Name.Text == a.Name && outer.Outer != null && outer.Outer.Name.Text == (a.Outer?.Name.Text ?? "");

    JObject Brief(UObject a, string cls, Xf? xf, string src, string? note)
    {
        var j = new JObject { ["name"] = a.Name, ["class"] = cls, ["source"] = src };
        if (ActorLabel(a) is string lb) j["label"] = lb;
        if (xf != null) j["location"] = xf.Value.ToJson()["location"];
        if (note != null) j["note"] = note;
        return j;
    }

    JArray ChildActorInfo(List<UObject> comps)
    {
        var arr = new JArray();
        foreach (var c in comps.Where(c => c.ExportType == "ChildActorComponent"))
        {
            ChainLookup(c, "ChildActorClass", out var cc);
            ChainLookup(c, "ChildActor", out var ca);
            arr.Add(new JObject { ["component"] = c.Name, ["child_actor_class"] = cc, ["child_actor"] = ca });
        }
        return arr;
    }

    JObject WeaponOverrides(UObject a)
    {
        var j = new JObject();
        var props = PropsJson(a, true);
        foreach (var p in props.Properties())
            if (Regex.IsMatch(p.Name, "Passport|Class|Weapon|Override|Material|Mesh|Part|Blade|Handle|Guard|Pommel", RegexOptions.IgnoreCase))
                j[p.Name] = p.Value;
        return j;
    }

    // ------------------------------------------------------------ transform helpers
    UObject? RootComp(UObject a)
    {
        var rc = Prop<FPackageIndex>(a, "RootComponent");
        if (rc == null || rc.IsNull) return null;
        return rc.Load();
    }

    Xf? ActorWorldXf(UObject a, Xf levelXf, out string? attach)
    {
        attach = null;
        var rc = RootComp(a);
        if (rc == null) return null;
        return CompWorldXf(rc, levelXf, 0, ref attach);
    }

    Xf CompWorldXf(UObject c, Xf levelXf, int depth, ref string? attach)
    {
        var rel = Xf.Identity;
        var loc = ChainGet<FVector>(c, "RelativeLocation");
        var rot = ChainGet<FRotator>(c, "RelativeRotation");
        var scl = ChainGet<FVector>(c, "RelativeScale3D");
        if (loc != null) rel.T = new V3(loc.Value.X, loc.Value.Y, loc.Value.Z);
        if (rot != null) rel.R = Quat.FromRotator(rot.Value.Pitch, rot.Value.Yaw, rot.Value.Roll);
        if (scl != null) rel.S = new V3(scl.Value.X, scl.Value.Y, scl.Value.Z);
        bool absL = ChainGetBool(c, "bAbsoluteLocation"), absR = ChainGetBool(c, "bAbsoluteRotation"), absS = ChainGetBool(c, "bAbsoluteScale");

        var ap = Prop<FPackageIndex>(c, "AttachParent");
        if (ap != null && !ap.IsNull && depth < 16)
        {
            var parent = ap.Load();
            if (parent != null)
            {
                if (depth == 0) attach = $"{parent.Outer?.Name}.{parent.Name}";
                string? dummy = null;
                var pxf = CompWorldXf(parent, levelXf, depth + 1, ref dummy);
                // socket attachment not resolvable statically (needs skeleton); noted via attach string
                return Xf.Compose(rel, pxf, absL, absR, absS);
            }
        }
        return Xf.Compose(rel, levelXf, absL, absR, absS);
    }

    // ------------------------------------------------------------ property helpers
    static T? Prop<T>(UObject o, string name)
    {
        try { return o.GetOrDefault<T>(name); } catch { return default; }
    }
    static bool TryProp<T>(UObject o, string name, out T val)
    {
        try { return o.TryGetValue(out val!, name); } catch { val = default!; return false; }
    }

    static IEnumerable<UObject> TemplateChain(UObject o)
    {
        var cur = o; int n = 0;
        while (cur != null && n++ < 12)
        {
            yield return cur;
            UObject? next = null;
            try { next = cur.Template?.Load(); } catch { }
            if ((next == null || next == cur) && cur.Name.StartsWith("Default__"))
            {
                try
                {
                    var c = cur.Class?.Load() as UStruct;
                    var sup = c?.SuperStruct?.Load() as UClass;
                    next = sup?.ClassDefaultObject?.Load();
                }
                catch { next = null; }
            }
            if (next == null || next == cur) yield break;
            cur = next;
        }
    }

    static T? ChainGet<T>(UObject o, string name) where T : struct
    {
        foreach (var t in TemplateChain(o))
            if (TryProp<T>(t, name, out var v)) return v;
        return null;
    }
    static bool ChainGetBool(UObject o, string name)
    {
        foreach (var t in TemplateChain(o))
            if (TryProp<bool>(t, name, out var v)) return v;
        return false;
    }

    bool ChainLookup(UObject o, string name, out JToken? val)
    {
        foreach (var t in TemplateChain(o))
        {
            var j = PropsJson(t, false);
            if (j.TryGetValue(name, out var v)) { val = v; return true; }
        }
        val = null; return false;
    }

    bool? StructFieldLookup(UObject o, string structName, string field)
    {
        foreach (var t in TemplateChain(o))
        {
            var j = PropsJson(t, false);
            if (j[structName] is JObject s && s.TryGetValue(field, out var v)) return v.Value<bool>();
        }
        return null;
    }

    string? MeshOf(UObject c)
    {
        foreach (var n in new[] { "StaticMesh", "SkeletalMesh", "SkinnedAsset", "SkeletalMeshAsset" })
            if (ChainLookup(c, n, out var v) && v != null && v.Type != JTokenType.Null) return v.ToString();
        return null;
    }

    static readonly HashSet<string> SkipProps = new()
    {
        "RootComponent", "BlueprintCreatedComponents", "InstanceComponents", "UCSSerializationIndex", "bNetAddressable",
        "CreationMethod", "AttachParent", "ActorLabel", "FolderPath", "SpawnCollisionHandlingMethod",
    };

    static readonly Dictionary<UObject, JObject> PropCache = new(ReferenceEqualityComparer.Instance);

    JObject PropsJson(UObject o, bool skipComponentRefs)
    {
        if (!PropCache.TryGetValue(o, out var props))
        {
            props = new JObject();
            try
            {
                var full = JObject.FromObject(o, JsonSerializer.Create(new JsonSerializerSettings { ReferenceLoopHandling = ReferenceLoopHandling.Ignore }));
                if (full["Properties"] is JObject p) props = (JObject)Simplify(p);
            }
            catch (Exception ex) { props["__error"] = ex.Message; }
            PropCache[o] = props;
        }
        if (!skipComponentRefs) return props;
        var r = new JObject();
        foreach (var p in props.Properties())
        {
            if (SkipProps.Contains(p.Name) || p.Name == "UberGraphFrame" || p.Name.EndsWith("_RefProperty")) continue;
            if (IsSubobjectRef(p.Value)) continue;
            r[p.Name] = p.Value;
        }
        return r;
    }

    // value is (an array of) references to subobjects (components) of some level actor / class template
    static bool IsSubobjectRef(JToken t) => t switch
    {
        JValue v when v.Type == JTokenType.String => Regex.IsMatch((string)v!, @"^\w+'[^']*(PersistentLevel\.[^.']+\.[^']+|_C:[^']+)'$") && Regex.IsMatch((string)v!, @"^\w*(Component|Scene\w*)'|Component'"),
        JArray a => a.Count > 0 && a.All(IsSubobjectRef),
        _ => false,
    };

    static JObject Slim(JObject eff)
    {
        var r = new JObject();
        var omitted = new JArray();
        foreach (var p in eff.Properties())
        {
            var js = p.Value.ToString(Formatting.None);
            if (js.Length > 1500 && !HasClassRefToken(p.Value)) { omitted.Add(p.Name); continue; }
            r[p.Name] = p.Value;
        }
        if (omitted.Count > 0) r["__omitted_large_defaults"] = omitted;
        return r;
    }

    JToken Simplify(JToken t)
    {
        switch (t)
        {
            case JObject o:
                if (o.Count <= 2 && o.ContainsKey("ObjectName") && o.ContainsKey("ObjectPath"))
                    return new JValue(ObjRefString((string?)o["ObjectName"], (string?)o["ObjectPath"]));
                if (o.Count <= 2 && o.ContainsKey("AssetPathName") && o.ContainsKey("SubPathString"))
                    return new JValue((string?)o["AssetPathName"] + ((string?)o["SubPathString"] is { Length: > 0 } sp ? ":" + sp : ""));
                var n = new JObject();
                foreach (var p in o.Properties()) n[p.Name] = Simplify(p.Value);
                return n;
            case JArray a:
                return new JArray(a.Select(Simplify));
            case JValue v when v.Type == JTokenType.String:
                return new JValue(EnumPretty((string)v!));
            default:
                return t;
        }
    }

    static string ObjRefString(string? name, string? path)
    {
        // "BlueprintGeneratedClass'X_C'" + "HalfswordUE5/Content/.../X.123" -> "/Game/.../X.X_C"
        if (name == null) return "null";
        var m = Regex.Match(name, @"^(\w+)'(.*)'$");
        if (!m.Success) return name;
        var type = m.Groups[1].Value; var obj = m.Groups[2].Value;
        if (path != null && !obj.Contains(':') && (type.Contains("Class") || !obj.Contains('.')))
        {
            var pkg = Regex.Replace(path, @"\.\d+$", "");
            return $"{type}'{ToGamePath(pkg)}.{obj}'";
        }
        return name;
    }

    static string ToGamePath(string p) => p.StartsWith("HalfswordUE5/Content/") ? "/Game/" + p["HalfswordUE5/Content/".Length..] : p;

    // ------------------------------------------------------------ enums
    string EnumPretty(string s)
    {
        var m = Regex.Match(s, @"^(\w+)::(NewEnumerator\d+)$");
        if (!m.Success) return s;
        var map = LoadUserEnum(m.Groups[1].Value);
        return map != null && map.TryGetValue(m.Groups[2].Value, out var disp) ? $"{s} ({disp})" : s;
    }

    Dictionary<string, string>? LoadUserEnum(string enumName)
    {
        if (EnumCache.TryGetValue(enumName, out var c)) return c;
        Dictionary<string, string>? res = null;
        try
        {
            var f = _p.Files.Keys.FirstOrDefault(k => k.EndsWith("/" + enumName + ".uasset", StringComparison.OrdinalIgnoreCase));
            if (f != null)
            {
                var pkg = _p.LoadPackage(f);
                var e = pkg.GetExports().FirstOrDefault(x => x.ExportType == "UserDefinedEnum");
                if (e != null)
                {
                    var j = JObject.FromObject(e);
                    res = new();
                    // DisplayNameMap: [{Key:"NewEnumerator0", Value:{...SourceString/LocalizedString}}]
                    if (j["Properties"]?["DisplayNameMap"] is JArray dm)
                        foreach (var kv in dm)
                        {
                            var key = (string?)kv["Key"]; var val = kv["Value"];
                            var txt = (string?)val?["LocalizedString"] ?? (string?)val?["SourceString"] ?? (string?)val?["TextData"]?["SourceString"] ?? val?.ToString();
                            if (key != null && txt != null) res[key.Contains("::") ? key.Split("::")[1] : key] = txt;
                        }
                }
            }
        }
        catch { }
        EnumCache[enumName] = res!;
        return res;
    }

    // ------------------------------------------------------------ class info
    List<string> ClassChain(UObject o)
    {
        var cls = o.Class;
        var key = cls?.GetPathName() ?? o.ExportType;
        if (ClassChainCache.TryGetValue(key, out var cached)) return cached;
        var chain = new List<string>();
        try
        {
            UStruct? cur = cls?.Load() as UStruct;
            int n = 0;
            while (cur != null && n++ < 40)
            {
                chain.Add(cur.Name);
                UStruct? next = null;
                try { next = cur.SuperStruct?.Load<UStruct>(); } catch { }
                if (next == null)
                {
                    // native class: continue via usmap super types
                    var nm = cur.Name;
                    var types = _p.MappingsForGame?.Types;
                    while (types != null && types.TryGetValue(nm, out var st) && !string.IsNullOrEmpty(st.SuperType) && n++ < 40)
                    {
                        nm = st.SuperType!;
                        chain.Add(nm);
                    }
                    break;
                }
                cur = next;
            }
        }
        catch { }
        if (chain.Count == 0) chain.Add(o.ExportType);
        ClassChainCache[key] = chain;
        return chain;
    }

    public JObject EffectivePropsPublic(UObject a) => EffectiveProps(a);

    JObject EffectiveProps(UObject a)
    {
        var res = new JObject();
        foreach (var t in TemplateChain(a))
        {
            var j = PropsJson(t, true);
            foreach (var p in j.Properties())
                if (!res.ContainsKey(p.Name)) res[p.Name] = p.Value;
        }
        return res;
    }

    bool HasClassRefs(UObject a)
    {
        var j = EffectiveProps(a);
        return HasClassRefToken(j);
    }
    static bool HasClassRefToken(JToken t) => t switch
    {
        JObject o => o.Properties().Any(p => HasClassRefToken(p.Value)),
        JArray arr => arr.Any(HasClassRefToken),
        JValue v when v.Type == JTokenType.String => Regex.IsMatch((string)v!, @"(BlueprintGeneratedClass'|_C'$|\._C$|\.\w+_C$)"),
        _ => false,
    };
}
