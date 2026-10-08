// Read-only inventory closure from the existing mounted provider. Authentication
// stays exclusively in Program.Init; neither keys nor provider internals serialize.
using System.Collections;
using System.Reflection;
using System.Security.Cryptography;
using System.Text;
using System.Text.RegularExpressions;
using CUE4Parse.FileProvider;
using CUE4Parse.UE4.Assets.Exports;
using CUE4Parse.UE4.Objects.UObject;
using Newtonsoft.Json;
using Newtonsoft.Json.Linq;

namespace MapDump;

public static class InventoryHarvester
{
    const int Schema = 1;
    static readonly Regex SensitiveHex = new(@"(?i)(?:0x)?[0-9a-f]{64}", RegexOptions.Compiled);
    static readonly Regex PackagePath = new(@"(?:/Game/|/Engine/|HalfswordUE5/Content/|Engine/Content/)[^\s'""(),;]+", RegexOptions.Compiled);
    static readonly Regex RootPaths = new(@"/Assets/(?:Weapons|Armor|Clothing|Equipment|Collisions|Materials)/|/Blueprints/(?:DataAssets|Enumerator|Struct[^/]*)/|/Curves/|/Physics/|/PhysicalMaterials/|/Materials/|/Character/Blueprints/|/Character/Skeleton/|/GameLogic/Inventory/|/GI_[^/]+|/GameInstance[^/]*", RegexOptions.IgnoreCase | RegexOptions.Compiled);

    public static bool InitializeQuietly(Action initialize)
    {
        var output = Console.Out; var error = Console.Error;
        try
        {
            // Bootstrap diagnostics may contain provider authentication details.
            Console.SetOut(TextWriter.Null); Console.SetError(TextWriter.Null);
            initialize(); return true;
        }
        catch (Exception ex)
        {
            output.WriteLine("Inventory provider initialization failed: " + ex.GetType().Name);
            return false;
        }
        finally { Console.SetOut(output); Console.SetError(error); }
    }

    public static int Run(DefaultFileProvider provider, string outputDirectory)
    {
        provider.ReadScriptData = true;
        var index = provider.Files.Keys.Where(IsPackage).Order(StringComparer.Ordinal).ToArray();
        Directory.CreateDirectory(outputDirectory);
        File.WriteAllLines(Path.Combine(outputDirectory, "all-package-index.txt"), index, new UTF8Encoding(false));
        var manifest = Harvest(index, package => Extract(provider, package), outputDirectory);
        manifest["source"] = SourceIdentity();
        File.WriteAllText(Path.Combine(outputDirectory, "coverage-manifest.json"), manifest.ToString(Formatting.Indented), new UTF8Encoding(false));
        var totals = (JObject)manifest["totals"]!;
        Console.WriteLine($"Inventory packages: listed={totals["listed"]}, success={totals["success"]}, failure={totals["failure"]}, explicit_skip={totals["explicit_skip"]}");
        Console.WriteLine($"References: strong_missing={manifest["missing_references"]!.Count()}, weak_unindexed={manifest["unindexed_reference_candidates"]!.Count()}, native_script={manifest["native_script_references"]!.Count()}");
        // A completed accounting manifest is retained on partial extraction.
        return totals["failure"]!.Value<int>() == 0 ? 0 : 2;
    }

    static bool IsPackage(string p) => p.EndsWith(".uasset", StringComparison.OrdinalIgnoreCase) || p.EndsWith(".umap", StringComparison.OrdinalIgnoreCase);
    public static bool IsRoot(string p) => p.StartsWith("HalfswordUE5/Content/", StringComparison.OrdinalIgnoreCase) && RootPaths.IsMatch(p);
    public static string FileName(string package)
    {
        var stem = Regex.Replace(Path.GetFileNameWithoutExtension(package), @"[^A-Za-z0-9_.-]", "_");
        if (stem.Length > 80) stem = stem[..80];
        return Convert.ToHexString(SHA256.HashData(Encoding.UTF8.GetBytes(package)))[..24].ToLowerInvariant() + "-" + stem + ".json";
    }

    public static string? NormalizeReference(string reference)
    {
        var match = PackagePath.Match(reference);
        if (!match.Success) return null;
        var p = match.Value.Replace('\\', '/');
        if (p.StartsWith("/Game/")) p = "HalfswordUE5/Content/" + p[6..];
        else if (p.StartsWith("/Engine/Content/")) p = "Engine/" + p[8..];
        else if (p.StartsWith("/Engine/")) p = "Engine/Content/" + p[8..];
        var colon = p.IndexOf(':'); if (colon >= 0) p = p[..colon];
        // Explicit cooked extensions are retained; object suffixes (.3/.Class_C)
        // identify exports inside a package, never a separately guessed package.
        if (!IsPackage(p))
        {
            var dot = p.IndexOf('.'); if (dot >= 0) p = p[..dot];
            p += ".uasset";
        }
        return p;
    }

    static string? ResolveReference(string reference, IReadOnlyDictionary<string, string> index)
    {
        var normalized = NormalizeReference(reference);
        if (normalized == null) return null;
        if (index.TryGetValue(normalized, out var exact)) return exact;
        // Export object paths omit the asset/map extension. Only an actually
        // indexed map authorizes this alternate resolution; explicit .uasset
        // references remain missing when that asset is absent.
        if (normalized.EndsWith(".uasset", StringComparison.OrdinalIgnoreCase)
            && !Regex.IsMatch(reference, @"\.uasset(?:$|[.'""\s:])", RegexOptions.IgnoreCase)
            && index.TryGetValue(normalized[..^7] + ".umap", out var map)) return map;
        return normalized;
    }

    static string SafeError(Exception e) => SensitiveHex.Replace(e.GetType().Name + ": " + e.Message, "[redacted]").Replace('\r', ' ').Replace('\n', ' ');
    static JObject SourceIdentity()
    {
        var game = Environment.GetEnvironmentVariable("HSMP_GAME_DIR") ?? Path.Combine(Directory.GetCurrentDirectory(), "game");
        var paks = Path.Combine(game, "HalfswordUE5", "Content", "Paks");
        var files = new JArray();
        foreach (var path in Directory.GetFiles(paks).Where(p => new[] { ".pak", ".utoc", ".ucas" }.Contains(Path.GetExtension(p), StringComparer.OrdinalIgnoreCase)).Order(StringComparer.Ordinal))
        {
            var f = new FileInfo(path);
            files.Add(new JObject { ["file"] = f.Name, ["bytes"] = f.Length, ["modified_utc"] = f.LastWriteTimeUtc,
                ["fingerprint_kind"] = "size_and_mtime; encrypted contents not hashed" });
        }
        return new JObject { ["archives"] = files, ["mount_scope"] = "top-level game Paks; LogicMods excluded by existing initialization",
            ["parser_version"] = typeof(UObject).Assembly.GetName().Version?.ToString(), ["game_writes"] = false,
            ["authentication"] = "existing initialization only; omitted from outputs" };
    }
    static object? Member(object o, string name)
    {
        var t = o.GetType();
        return t.GetProperty(name, BindingFlags.Public | BindingFlags.Instance)?.GetValue(o)
            ?? t.GetField(name, BindingFlags.Public | BindingFlags.Instance)?.GetValue(o);
    }
    static int? ExpectedExports(object package)
    {
        foreach (var name in new[] { "ExportMapLength", "ExportsLazy", "ExportMap" })
        {
            var v = Member(package, name);
            if (v is int n) return n;
            if (v is Array a) return a.Length;
            if (v is ICollection c) return c.Count;
        }
        return null;
    }

    static JObject Extract(DefaultFileProvider provider, string package)
    {
        var exports = new JArray(); var errors = new JArray(); var names = new JArray(); var layers = new JArray();
        var originalOut = Console.Out; var originalError = Console.Error;
        using var diagnostics = new StringWriter();
        int? expected = null; int visited = 0;
        try
        {
            Console.SetOut(diagnostics); Console.SetError(diagnostics);
            var parsed = provider.LoadPackage(package);
            expected = ExpectedExports(parsed);
            var nameMap = Member(parsed, "NameMap") as IEnumerable;
            if (nameMap != null)
                foreach (var name in nameMap)
                {
                    var value = Member(name!, "Name") as string ?? name!.ToString();
                    if (value != null && (NormalizeReference(value) != null || value.Contains("/Script/"))) names.Add(value);
                }
            // GetExports can throw during lazy iteration. Keep every previously
            // parsed export but classify the package as a failure, not success.
            foreach (var export in parsed.GetExports())
            {
                visited++;
                try
                {
                    exports.Add(JObject.Parse(JsonConvert.SerializeObject(export, Formatting.None)));
                    if (export.Name.StartsWith("Default__", StringComparison.Ordinal))
                        layers.Add(TemplateLayers(export));
                }
                catch (Exception ex) { errors.Add(new JObject { ["stage"] = "export", ["export"] = export.Name, ["error"] = SafeError(ex) }); }
            }
        }
        catch (Exception ex) { errors.Add(new JObject { ["stage"] = "package_or_lazy_export", ["error"] = SafeError(ex) }); }
        finally { Console.SetOut(originalOut); Console.SetError(originalError); }
        if (expected == null) errors.Add(new JObject { ["stage"] = "coverage", ["error"] = "native export count unavailable" });
        else if (expected != visited) errors.Add(new JObject { ["stage"] = "coverage", ["error"] = $"expected {expected}, visited {visited}" });
        if (expected == 0) errors.Add(new JObject { ["stage"] = "coverage", ["error"] = "zero native exports; inventory content unavailable" });
        // CUE4Parse can log an unsupported property and continue. Never silently
        // promote those diagnostics to a clean, complete serialized package.
        var messages = SensitiveHex.Replace(diagnostics.ToString(), "[redacted]");
        if (!string.IsNullOrWhiteSpace(messages))
            errors.Add(new JObject { ["stage"] = "parser_diagnostics", ["error"] = messages.Length > 16000 ? messages[..16000] : messages, ["truncated"] = messages.Length > 16000 });
        return new JObject
        {
            ["schema"] = Schema, ["package"] = package, ["expected_exports"] = expected,
            ["visited_exports"] = visited, ["serialized_exports"] = exports.Count,
            ["complete"] = errors.Count == 0, ["errors"] = errors, ["reference_names"] = names, ["exports"] = exports, ["default_layers"] = layers,
            ["runtime_construction_executed"] = false,
            ["complex_collision_triangles"] = "unavailable_not_decoded",
            ["cooked_physics_binary"] = "unavailable_not_decoded"
        };
    }

    static JObject TemplateLayers(UObject initial)
    {
        var layers = new JArray(); var seen = new HashSet<string>(); var current = initial;
        string terminal = "unavailable"; bool complete = false; JObject? nativeParent = null;
        try
        {
            while (current != null && layers.Count < 40)
            {
                var identity = current.GetPathName();
                if (!seen.Add(identity)) { terminal = "archetype_cycle"; break; }
                var json = JObject.Parse(JsonConvert.SerializeObject(current, Formatting.None));
                layers.Add(new JObject { ["object"] = identity, ["class"] = current.ExportType,
                    ["properties"] = json["Properties"]?.DeepClone() ?? new JObject() });
                UObject? next = current.Template?.Load();
                if (next == current) next = null;
                if (next == null && current.Name.StartsWith("Default__", StringComparison.Ordinal))
                {
                    var cls = current.Class?.Load() as UStruct;
                    if (cls == null) { terminal = "class_unavailable"; break; }
                    var super = cls?.SuperStruct?.Load() as UClass;
                    if (super == null) { terminal = "superclass_unavailable"; break; }
                    var serializedSuper = JObject.Parse(JsonConvert.SerializeObject(cls, Formatting.None))["SuperStruct"];
                    if (ResolvedNativeParent(super.GetPathName(), super.Name, serializedSuper))
                    {
                        nativeParent = new JObject { ["reference"] = serializedSuper?.DeepClone(),
                            ["loaded_name"] = super.Name, ["loaded_object"] = super.GetPathName(),
                            ["loaded_type"] = super.GetType().Name };
                        terminal = "native_parent_defaults_unavailable"; complete = true; break;
                    }
                    next = super.ClassDefaultObject?.Load();
                    if (next == null) { terminal = "parent_CDO_unavailable"; break; }
                }
                if (next == null) { terminal = "template_unavailable"; break; }
                current = next;
            }
            if (layers.Count == 40) terminal = "archetype_limit";
        }
        catch (Exception ex) { terminal = SafeError(ex); }
        return new JObject { ["export"] = initial.Name, ["class"] = initial.ExportType, ["layers"] = layers,
            ["game_ancestry_available"] = complete, ["terminal"] = terminal, ["native_defaults_available"] = false,
            ["native_parent"] = nativeParent,
            ["runtime_construction_executed"] = false, ["struct_merge"] = "first present outer property; omitted nested fields stay unavailable" };
    }
    static bool ResolvedNativeParent(string? path, string? loadedName = null, JToken? reference = null)
    {
        if (path?.StartsWith("/Script/", StringComparison.Ordinal) == true) return true;
        // CUE4Parse's loaded native class path may omit its script package.
        // Require both the serialized native package and matching loaded class
        // identity. A null/unreadable superclass never reaches this predicate.
        var objectName = (string?)reference?["ObjectName"];
        return loadedName != null && ((string?)reference?["ObjectPath"])?.StartsWith("/Script/", StringComparison.Ordinal) == true
            && objectName == "Class'" + loadedName + "'";
    }

    static IEnumerable<(string Value, string Path)> Strings(JToken token, string path = "")
    {
        if (token is JObject o)
            foreach (var property in o.Properties())
                foreach (var value in Strings(property.Value, path + "/" + property.Name)) yield return value;
        else if (token is JArray a)
            for (var i = 0; i < a.Count; i++)
                foreach (var value in Strings(a[i], path + "/" + i)) yield return value;
        else if (token.Type == JTokenType.String) yield return (token.Value<string>()!, path);
    }

    static string HumanField(string key) => Regex.Replace(key, @"_\d+_[0-9A-Fa-f]{32}$", "");
    static JToken Humanize(JToken value)
    {
        if (value is JObject o)
        {
            var result = new JObject();
            var duplicates = o.Properties().GroupBy(p => HumanField(p.Name)).Where(g => g.Count() > 1).Select(g => g.Key).ToHashSet();
            foreach (var p in o.Properties()) result[duplicates.Contains(HumanField(p.Name)) ? p.Name : HumanField(p.Name)] = Humanize(p.Value);
            return result;
        }
        return value is JArray a ? new JArray(a.Select(Humanize)) : value.DeepClone();
    }
    static IEnumerable<(string Name, string Path, JToken Value)> Fields(JToken value, string path = "")
    {
        if (value is JObject o)
            foreach (var p in o.Properties())
            {
                var name = HumanField(p.Name); var current = path + "/" + p.Name;
                yield return (name, current, p.Value);
                foreach (var child in Fields(p.Value, current)) yield return child;
            }
        else if (value is JArray a)
            for (var i = 0; i < a.Count; i++)
                foreach (var child in Fields(a[i], path + "/" + i)) yield return child;
    }
    static bool EligibleStatField(string path)
    {
        var parts = path.Split('/', StringSplitOptions.RemoveEmptyEntries);
        if (parts.Length == 1) return true;
        // Only direct class properties and these native gear structs have
        // established stat semantics. Transform/quaternion Size and arbitrary
        // component metadata stay in observed_defaults, never canonical stats.
        return parts.Length == 2 && HumanField(parts[0]) is "Armor Passport" or "Weapon Passport"
            or "Protection Core" or "Protection Module 1" or "Protection Module 2" or "Protection Module 3";
    }
    static string GearFamily(string package, string name, JObject? inheritance)
    {
        // Native ancestry identifies animation instances before their package's
        // gear folder can incorrectly classify them as wearable armor/weapons.
        if ((string?)inheritance?["native_parent"]?["loaded_name"] == "AnimInstance")
            return package.Contains("/Armor/", StringComparison.OrdinalIgnoreCase) ? "armor_animation"
                : package.Contains("/Weapons/", StringComparison.OrdinalIgnoreCase) ? "weapon_animation" : "animation";
        if (package.Contains("/Armor/Blueprints/Built_Armor/Cloth/", StringComparison.OrdinalIgnoreCase)) return "clothing";
        if (package.Contains("/Armor/", StringComparison.OrdinalIgnoreCase)) return name.Contains("_Module_") ? "armor_module" : name.Contains("_Core_") ? "armor_core" : "armor";
        if (package.Contains("/Clothing/", StringComparison.OrdinalIgnoreCase)) return "clothing";
        if (package.Contains("/Weapons/", StringComparison.OrdinalIgnoreCase))
        {
            if (new[] { "Modular_Weapon_Grip_C", "Modular_Weapon_Module_C", "Modular_Weapon_Part_Master_C", "Modular_Weapon_SubModule_C" }.Contains(name)) return "weapon_module_base";
            return package.Contains("/Modules/", StringComparison.OrdinalIgnoreCase) ? "weapon_module" : package.Contains("/Built_Weapons/", StringComparison.OrdinalIgnoreCase) ? "built_weapon" : "weapon_base_or_other";
        }
        return "equipment";
    }
    static JObject EnumValue(JToken value, JArray enums)
    {
        var result = new JObject { ["symbol"] = value.DeepClone(), ["numeric_available"] = false };
        if (value.Type != JTokenType.String || !value.Value<string>()!.Contains("::")) return result;
        var symbol = value.Value<string>()!;
        var enumName = symbol.Split("::")[0];
        var native = enums.OfType<JObject>().FirstOrDefault(e => (string?)e["export"] == enumName);
        var names = native?["names"];
        JToken? numeric = (names as JObject)?[symbol];
        if (names is JArray pairs)
            foreach (var pair in pairs.OfType<JObject>())
                if ((string?)pair["Name"] == symbol || (string?)pair["Key"] == symbol) numeric = pair["Value"];
        if (numeric?.Type == JTokenType.Integer)
        {
            result["numeric_available"] = true; result["numeric"] = numeric.DeepClone(); result["source_package"] = native!["package"];
        }
        // Display labels are separate from numeric identity and never convert
        // NewEnumeratorNN's suffix to a runtime enum value.
        if (native?["display_names"] is JArray display)
            foreach (var pair in display.OfType<JObject>())
                if ((string?)pair["Key"] == symbol || (string?)pair["Key"] == symbol.Split("::")[1])
                    result["display"] = pair["Value"]?.DeepClone();
        return result;
    }
    public static JObject Catalogue(JArray defaults, JArray enums)
    {
        var rows = new JArray();
        var required = new Dictionary<string, string[]>
        {
            ["slash_protection"] = new[] { "Protection Cut", "Cut Protection", "Prot Cut", "Protection Slash", "Slash Protection", "Prot Slash", "Protection Slash Default" },
            ["stab_protection"] = new[] { "Protection Stab", "Stab Protection", "Prot Stab", "Protection Stab Default" },
            ["blunt_protection"] = new[] { "Protection Blunt", "Blunt Protection", "Prot Blunt", "Protection Blunt Default" },
            ["density"] = new[] { "Material Density", "Density", "Material Density Default" },
            ["mass"] = new[] { "Mass", "Weight", "Weapon Weight", "Armor Weight" },
            ["size"] = new[] { "Size", "Weapon Size", "Blade Size" },
            ["steel"] = new[] { "SteelType", "Steel Type" },
            ["material"] = new[] { "Material", "Material Type", "Wood Type" },
            ["tier"] = new[] { "Tier", "Rank" },
            ["price"] = new[] { "Price" }
        };
        foreach (var cdo in defaults.OfType<JObject>().Where(c => Regex.IsMatch((string)c["package"]!, @"/Assets/(Weapons|Armor|Clothing|Equipment)/", RegexOptions.IgnoreCase)))
        {
            var package = (string)cdo["package"]!; var name = (string)cdo["class"]!;
            var inheritance = cdo["inheritance"] as JObject;
            var layers = inheritance?["layers"] as JArray ?? new JArray(new JObject
                { ["object"] = package + "::" + cdo["export"], ["properties"] = cdo["properties"] });
            var selected = new JObject(); var provenance = new JObject();
            // Matches the existing extractor's outer-property precedence. A
            // partially serialized struct is NOT silently completed from a parent.
            foreach (var layer in layers.OfType<JObject>())
                foreach (var property in (layer["properties"] as JObject ?? new JObject()).Properties())
                    if (!selected.ContainsKey(property.Name))
                    {
                        selected[property.Name] = property.Value.DeepClone();
                        provenance[property.Name] = layer["object"]?.DeepClone();
                    }
            var stats = new JObject();
            foreach (var (stat, aliases) in required)
            {
                var candidates = Fields(selected).Where(f => EligibleStatField(f.Path) && aliases.Any(alias => string.Equals(Regex.Replace(alias, @"[\s._]", ""), Regex.Replace(f.Name, @"[\s._]", ""), StringComparison.OrdinalIgnoreCase))).ToArray();
                var field = candidates.FirstOrDefault();
                var present = candidates.Length == 1;
                var sourceProperty = present ? field.Path.Split('/')[1] : null;
                stats[stat] = new JObject { ["available"] = present, ["value"] = present ? Humanize(field.Value) : null,
                    ["source_field"] = present ? field.Path : null, ["source_object"] = sourceProperty != null ? provenance[sourceProperty]?.DeepClone() : null,
                    ["provenance"] = present ? "serialized_CDO_or_archetype; construction_not_executed" : "not_serialized_or_runtime_constructed",
                    ["native_alias"] = present ? field.Name : null,
                    ["unavailable_reason"] = candidates.Length > 1 ? "ambiguous_distinct_native_fields" : present ? null : "not_observed",
                    ["candidates"] = new JArray(candidates.Select(c => new JObject { ["field"] = c.Path, ["native_name"] = c.Name,
                        ["value"] = Humanize(c.Value), ["source_object"] = provenance[c.Path.Split('/')[1]]?.DeepClone() })),
                    ["runtime_value_available"] = false };
                if (present && field.Value.Type == JTokenType.String && field.Value.Value<string>()!.Contains("::"))
                    stats[stat]!["enum"] = EnumValue(field.Value, enums);
            }
            rows.Add(new JObject
            {
                ["package"] = package, ["class"] = name, ["family"] = GearFamily(package, name, inheritance),
                ["display_name"] = name.EndsWith("_C") ? name[..^2].Replace('_', ' ') : name.Replace('_', ' '),
                ["display_name_source"] = "native_class_name", ["stats"] = stats,
                ["observed_defaults"] = Humanize(selected), ["property_provenance"] = provenance,
                ["game_ancestry_available"] = inheritance?["game_ancestry_available"] ?? false,
                ["inheritance_terminal"] = inheritance?["terminal"] ?? "unavailable",
                ["native_defaults_available"] = false, ["runtime_construction_executed"] = false,
                ["package_status"] = cdo["package_status"]
                ,["component_templates_file"] = "exports/" + FileName(package),
                ["component_geometry_available"] = false,
                ["component_geometry_reason"] = "raw SCS/BodySetup exports retained; runtime reconstruction not executed"
            });
        }
        var counts = new JObject();
        foreach (var g in rows.OfType<JObject>().GroupBy(r => (string)r["family"]!).OrderBy(g => g.Key)) counts[g.Key] = g.Count();
        counts["weapon_module_classes_including_bases"] = rows.Count(r => (string?)r["family"] is "weapon_module" or "weapon_module_base");
        counts["built_armor_including_clothing"] = rows.Count(r => ((string)r["package"]!).Contains("/Built_Armor/"));
        counts["all_armor_classes"] = rows.Count(r => ((string)r["package"]!).Contains("/Assets/Armor/") && (string?)r["family"] != "armor_animation");
        counts["animation_class_rows"] = rows.Count(r => ((string)r["family"]!).EndsWith("animation", StringComparison.Ordinal));
        counts["gear_class_rows"] = rows.Count(r => !((string)r["family"]!).EndsWith("animation", StringComparison.Ordinal));
        var availability = new JObject();
        foreach (var stat in required.Keys)
            availability[stat] = new JObject { ["serialized_available"] = rows.Count(r => (bool?)r["stats"]?[stat]?["available"] == true),
                ["unavailable"] = rows.Count(r => (bool?)r["stats"]?[stat]?["available"] != true), ["runtime_confirmed"] = 0 };
        return new JObject { ["schema"] = Schema, ["counts"] = counts, ["stat_availability"] = availability, ["items"] = rows,
            ["semantics"] = "all observed gear class CDOs; not all runtime Cartesian module combinations; serialized zero is not runtime zero",
            ["collision_geometry"] = "complex_triangles_unavailable; inspect raw mesh/BodySetup references and component properties",
            ["runtime_exhaustive_coverage"] = false };
    }

    public static int RefreshCatalogue(string directory)
    {
        // Derived-only refresh never initializes or reads the game provider.
        var defaults = JArray.Parse(File.ReadAllText(Path.Combine(directory, "class-cdo-overrides.json")));
        var enums = JArray.Parse(File.ReadAllText(Path.Combine(directory, "native-enums.json")));
        var catalogue = Catalogue(defaults, enums);
        File.WriteAllText(Path.Combine(directory, "canonical-inventory.json"), catalogue.ToString(Formatting.Indented), new UTF8Encoding(false));
        Console.WriteLine($"Inventory catalogue refreshed: rows={catalogue["items"]!.Count()}, gear={catalogue["counts"]!["gear_class_rows"]}, animation={catalogue["counts"]!["animation_class_rows"]}");
        return 0;
    }

    public static JObject Harvest(IEnumerable<string> listed, Func<string, JObject> extract, string? outputDirectory = null)
    {
        var index = listed.Distinct(StringComparer.OrdinalIgnoreCase).Order(StringComparer.Ordinal).ToArray();
        var canonical = index.ToDictionary(x => x, StringComparer.OrdinalIgnoreCase);
        var rows = index.ToDictionary(x => x, x => new JObject
        {
            ["package"] = x, ["status"] = "explicit_skip", ["reason"] = "outside_inventory_dependency_closure", ["root"] = IsRoot(x)
        }, StringComparer.OrdinalIgnoreCase);
        var queue = new Queue<string>(index.Where(IsRoot).OrderBy(x => x.Contains("/Armor/Blueprints/") ? 0 : 1).ThenBy(x => x, StringComparer.Ordinal));
        var queued = queue.ToHashSet(StringComparer.OrdinalIgnoreCase);
        var edges = new JArray(); var missing = new JArray(); var candidates = new JArray();
        var script = new HashSet<string>(StringComparer.Ordinal); var defaults = new JArray(); var classes = new JArray();
        var enums = new JArray();
        if (outputDirectory != null) Directory.CreateDirectory(Path.Combine(outputDirectory, "exports"));
        int completed = 0;
        while (queue.TryDequeue(out var package))
        {
            if (package.EndsWith(".umap", StringComparison.OrdinalIgnoreCase))
            {
                rows[package]["reason"] = "map_body_outside_inventory_scope";
                continue;
            }
            JObject result;
            try { result = extract(package); }
            catch (Exception ex) { result = new JObject { ["complete"] = false, ["errors"] = new JArray(SafeError(ex)), ["exports"] = new JArray() }; }
            var row = rows[package];
            row["status"] = result["complete"]?.Value<bool>() == true ? "success" : "failure";
            row["reason"] = result["complete"]?.Value<bool>() == true ? "all_native_exports_serialized" : "partial_or_unavailable_native_export";
            row["expected_exports"] = result["expected_exports"]; row["serialized_exports"] = result["serialized_exports"];
            row["errors"] = result["errors"];
            row["runtime_construction_executed"] = false;
            row["complex_collision_triangles"] = "unavailable_not_decoded";
            var file = "exports/" + FileName(package); row["file"] = file;
            if (outputDirectory != null) File.WriteAllText(Path.Combine(outputDirectory, file), result.ToString(Formatting.Indented), new UTF8Encoding(false));
            foreach (var (value, source) in Strings(result))
            {
                if (value.Contains("/Script/")) { script.Add(value); continue; }
                var reference = ResolveReference(value, canonical); if (reference == null || reference.Equals(package, StringComparison.OrdinalIgnoreCase)) continue;
                var kind = source.EndsWith("/ObjectPath", StringComparison.Ordinal) || source.EndsWith("/AssetPathName", StringComparison.Ordinal)
                    ? "serialized_object_reference" : source.StartsWith("/reference_names/", StringComparison.Ordinal) ? "name_table_candidate" : "serialized_path_literal";
                if (canonical.TryGetValue(reference, out var target))
                {
                    edges.Add(new JObject { ["from"] = package, ["to"] = target, ["source"] = source, ["reference"] = value, ["kind"] = kind });
                    if (queued.Add(target)) queue.Enqueue(target);
                }
                else
                {
                    var unknown = new JObject { ["from"] = package, ["to"] = reference, ["source"] = source, ["reference"] = value, ["kind"] = kind };
                    if (kind == "serialized_object_reference") missing.Add(unknown); else candidates.Add(unknown);
                }
            }
            foreach (var export in (result["exports"] as JArray ?? new JArray()).OfType<JObject>())
            {
                var name = export["Name"]?.Value<string>() ?? "";
                var type = export["Type"]?.Value<string>() ?? "";
                if (name.StartsWith("Default__", StringComparison.Ordinal))
                    defaults.Add(new JObject { ["package"] = package, ["export"] = name, ["class"] = type,
                        ["properties"] = export["Properties"]?.DeepClone() ?? new JObject(),
                        ["provenance"] = "serialized_class_CDO_overrides", ["runtime_construction_executed"] = false,
                        ["missing_property_semantics"] = "unavailable_not_zero", ["package_status"] = row["status"],
                        ["inheritance"] = (result["default_layers"] as JArray)?.OfType<JObject>().FirstOrDefault(x => (string?)x["export"] == name)?.DeepClone() });
                if (type is "BlueprintGeneratedClass" or "Class")
                    classes.Add(new JObject { ["package"] = package, ["class"] = name,
                        ["super"] = export["SuperStruct"]?.DeepClone(), ["default_object"] = export["ClassDefaultObject"]?.DeepClone(),
                        ["package_status"] = row["status"] });
                if (type is "UserDefinedEnum" or "Enum")
                    enums.Add(new JObject { ["package"] = package, ["export"] = name, ["names"] = export["Names"]?.DeepClone(),
                        ["display_names"] = export["Properties"]?["DisplayNameMap"]?.DeepClone(),
                        ["numeric_mapping"] = export["Names"] == null ? "unavailable" : "native_enum_Names; never enumerator_suffix" });
            }
            completed++;
            if (outputDirectory != null && completed % 50 == 0)
                Console.WriteLine($"Inventory extracted {completed}; queued remaining {queue.Count}; failures {rows.Values.Count(r => (string?)r["status"] == "failure")}");
        }
        var manifest = new JObject
        {
            ["schema"] = Schema, ["evidence"] = "offline cooked package serialization; no game/native construction",
            ["scope"] = "inventory roots and transitive serialized/name-table references; all effective mounted packages accounted",
            ["mount_semantics"] = "effective provider index; shadowed archive entries are not distinct runtime assets",
            ["index_sha256"] = Convert.ToHexString(SHA256.HashData(Encoding.UTF8.GetBytes(string.Join("\n", index)))).ToLowerInvariant(),
            ["totals"] = new JObject { ["listed"] = index.Length, ["success"] = rows.Values.Count(r => (string?)r["status"] == "success"),
                ["failure"] = rows.Values.Count(r => (string?)r["status"] == "failure"), ["explicit_skip"] = rows.Values.Count(r => (string?)r["status"] == "explicit_skip"),
                ["inventory_roots"] = index.Count(IsRoot), ["selected_closure"] = queued.Count },
            ["missing_references"] = missing, ["unindexed_reference_candidates"] = candidates,
            ["native_script_references"] = new JArray(script.Order(StringComparer.Ordinal)),
            ["packages"] = new JArray(index.Select(x => rows[x])),
            ["unselected_gear_like_paths"] = new JArray(index.Where(x => (string?)rows[x]["status"] == "explicit_skip"
                && Regex.IsMatch(x, @"Weapon|Armor|Cloth|Inventory|Equipment|PhysicalMaterial", RegexOptions.IgnoreCase))),
            ["dependencies"] = edges, ["complex_collision_triangles"] = "unavailable_not_decoded",
            ["runtime_exhaustive_coverage"] = false
        };
        if (outputDirectory != null)
        {
            File.WriteAllText(Path.Combine(outputDirectory, "coverage-manifest.json"), manifest.ToString(Formatting.Indented), new UTF8Encoding(false));
            File.WriteAllText(Path.Combine(outputDirectory, "class-cdo-overrides.json"), defaults.ToString(Formatting.Indented), new UTF8Encoding(false));
            File.WriteAllText(Path.Combine(outputDirectory, "class-ancestry.json"), classes.ToString(Formatting.Indented), new UTF8Encoding(false));
            File.WriteAllText(Path.Combine(outputDirectory, "native-enums.json"), enums.ToString(Formatting.Indented), new UTF8Encoding(false));
            var catalogue = Catalogue(defaults, enums);
            File.WriteAllText(Path.Combine(outputDirectory, "canonical-inventory.json"), catalogue.ToString(Formatting.Indented), new UTF8Encoding(false));
        }
        return manifest;
    }

    public static int SelfTest()
    {
        var checks = 0;
        void Check(bool pass, string name) { checks++; if (!pass) throw new InvalidOperationException("Inventory selftest: " + name); }
        const string armor = "HalfswordUE5/Content/Assets/Armor/Blueprints/Built_Armor/Test.uasset";
        const string material = "HalfswordUE5/Content/Elsewhere/SharedMaterial.uasset";
        const string failed = "HalfswordUE5/Content/Elsewhere/Failed.uasset";
        const string skipped = "HalfswordUE5/Content/Maps/Unrelated.uasset";
        var fake = new Dictionary<string, JObject>
        {
            [armor] = JObject.Parse("""{"complete":true,"expected_exports":2,"serialized_exports":2,"exports":[{"Name":"Default__Armor_C","Type":"Armor_C","Properties":{"Armor Passport":{"Steel":3},"Material":{"ObjectPath":"/Game/Elsewhere/SharedMaterial.SharedMaterial"}}},{"Name":"Armor_C","Type":"BlueprintGeneratedClass","SuperStruct":{"ObjectPath":"/Game/Elsewhere/Failed.1"}}]}"""),
            [material] = JObject.Parse("""{"complete":true,"exports":[{"Properties":{"Missing":{"ObjectPath":"/Game/Elsewhere/Missing.0"},"Native":"/Script/Engine.Material"}}]}"""),
            [failed] = JObject.Parse("""{"complete":false,"errors":["unsupported property"],"exports":[]}""")
        };
        var result = Harvest(new[] { armor, material, failed, skipped }, p => fake[p]);
        var totals = (JObject)result["totals"]!;
        Check(totals["listed"]!.Value<int>() == 4 && totals["success"]!.Value<int>() == 2
            && totals["failure"]!.Value<int>() == 1 && totals["explicit_skip"]!.Value<int>() == 1, "exact complete ledger identity");
        Check(result["dependencies"]!.Count() == 2, "non-root material and superclass reached by closure");
        Check(result["missing_references"]!.Count() == 1 && result["native_script_references"]!.Count() == 1, "missing and native references remain explicit");
        Check(NormalizeReference("Texture2D'/Game/A/B.B'") == "HalfswordUE5/Content/A/B.uasset", "typed soft object reference");
        Check(NormalizeReference("Engine/Content/A/B.0") == "Engine/Content/A/B.uasset", "cooked numeric export reference");
        Check(NormalizeReference("/Engine/Content/Slate/Common/Button.Button") == "Engine/Content/Slate/Common/Button.uasset", "filesystem-style engine alias never duplicates Content");
        const string mapPackage = "HalfswordUE5/Content/Maps/Test.umap";
        var mapIndex = new Dictionary<string, string>(StringComparer.OrdinalIgnoreCase) { [mapPackage] = mapPackage };
        Check(ResolveReference("/Game/Maps/Test.Test", mapIndex) == mapPackage, "extensionless indexed map reference resolves map, not missing asset");
        Check(ResolveReference("/Game/Maps/Test.uasset", mapIndex) == "HalfswordUE5/Content/Maps/Test.uasset", "explicit absent asset does not silently resolve a map");
        Check(FileName("A/B_C.uasset") != FileName("A_B/C.uasset"), "flattened file names cannot alias");
        Check(IsRoot("HalfswordUE5/Content/Assets/Clothing/Materials/MI_Doublet.uasset"), "clothing materials are never visual-only skipped");
        Check(IsRoot("HalfswordUE5/Content/Blueprints/DataAssets/Equipment/Tier.uasset"), "all inventory data assets seeded");
        Check(IsRoot("HalfswordUE5/Content/Assets/Collisions/Character/Body.uasset"), "collision roots seeded");
        Check((string?)result["complex_collision_triangles"] == "unavailable_not_decoded", "unavailable cooked triangles never claim completeness");
        Check((bool?)result["runtime_exhaustive_coverage"] == false, "offline closure is not runtime exhaustive proof");
        var fakeDefaults = new JArray(new JObject { ["package"] = armor, ["class"] = "Armor_C", ["properties"] = new JObject(),
            ["inheritance"] = new JObject { ["layers"] = new JArray(
                new JObject { ["object"] = "child", ["properties"] = new JObject { ["Price"] = 0, ["Armor Passport"] = new JObject { ["SteelType"] = "Steel_Type::NewEnumerator3" } } },
                new JObject { ["object"] = "parent", ["properties"] = new JObject { ["Protection Slash"] = 8.5, ["Price"] = 10 } }),
                ["game_ancestry_available"] = true, ["terminal"] = "native_parent_defaults_unavailable" } });
        var enumFixture = new JArray(new JObject { ["export"] = "Steel_Type", ["package"] = "native-enum",
            ["names"] = new JObject { ["Steel_Type::NewEnumerator3"] = 7 } });
        var catalog = Catalogue(fakeDefaults, enumFixture);
        var item = catalog["items"]![0]!;
        Check((double?)item["stats"]?["slash_protection"]?["value"] == 8.5
            && (string?)item["stats"]?["slash_protection"]?["source_object"] == "parent", "inherited native scalar retains parent provenance");
        Check((int?)item["stats"]?["price"]?["value"] == 0 && (string?)item["stats"]?["price"]?["source_object"] == "child", "real serialized zero overrides parent without being runtime proof");
        Check((bool?)item["stats"]?["density"]?["available"] == false && item["stats"]?["density"]?["value"]?.Type == JTokenType.Null, "missing density is unavailable rather than synthesized zero");
        Check((int?)item["stats"]?["steel"]?["enum"]?["numeric"] == 7, "native enum Names wins over enumerator suffix");
        Check((bool?)item["stats"]?["slash_protection"]?["runtime_value_available"] == false, "static inherited protection does not claim constructed runtime protection");
        ((JObject)fakeDefaults[0]!["inheritance"]!["layers"]![0]!["properties"]!)["Price"] = 0;
        ((JObject)fakeDefaults[0]!["inheritance"]!["layers"]![0]!["properties"]!["Armor Passport"]!)["Price"] = 15;
        var ambiguous = Catalogue(fakeDefaults, enumFixture)["items"]![0]!["stats"]!["price"]!;
        Check((bool?)ambiguous["available"] == false && ambiguous["candidates"]!.Count() == 2
            && ambiguous["value"]!.Type == JTokenType.Null, "different native field paths never silently choose one canonical stat");
        ((JObject)fakeDefaults[0]!["inheritance"]!["layers"]![0]!["properties"]!)["World Transform"] = JObject.Parse("""{"Rotation":{"Size":1}}""");
        var size = Catalogue(fakeDefaults, enumFixture)["items"]![0]!["stats"]!["size"]!;
        Check((bool?)size["available"] == false && size["candidates"]!.Count() == 0, "quaternion norm Size is not gear size");
        Check(!ResolvedNativeParent(null) && !ResolvedNativeParent("/Game/Unknown")
            && ResolvedNativeParent("/Script/Engine.Actor"), "only positively resolved native superclass terminates complete game ancestry");
        var actorReference = JObject.Parse("""{"ObjectName":"Class'Actor'","ObjectPath":"/Script/Engine"}""");
        Check(ResolvedNativeParent("Actor", "Actor", actorReference)
            && !ResolvedNativeParent("Actor", null, actorReference)
            && !ResolvedNativeParent("Actor", "Other", actorReference), "native script alias requires positively loaded matching class identity");
        var animation = new JObject { ["package"] = "HalfswordUE5/Content/Assets/Armor/Arm/Vambrace/Animation/ABP_CouterTransformCorrection.uasset",
            ["class"] = "ABP_CouterTransformCorrection_C", ["properties"] = new JObject(),
            ["inheritance"] = new JObject { ["native_parent"] = new JObject { ["loaded_name"] = "AnimInstance" } } };
        var animationCatalogue = Catalogue(new JArray(animation), new JArray());
        Check((string?)animationCatalogue["items"]![0]!["family"] == "armor_animation"
            && (int?)animationCatalogue["counts"]!["all_armor_classes"] == 0
            && (int?)animationCatalogue["counts"]!["gear_class_rows"] == 0
            && (int?)animationCatalogue["counts"]!["animation_class_rows"] == 1, "proven native animation in armor folder never counts as gear");
        Console.WriteLine($"Inventory selftest: {checks} checks passed");
        return 0;
    }
}
