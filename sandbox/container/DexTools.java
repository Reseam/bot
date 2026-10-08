import com.android.tools.smali.dexlib2.DexFileFactory;
import com.android.tools.smali.dexlib2.Opcodes;
import com.android.tools.smali.dexlib2.iface.ClassDef;
import com.android.tools.smali.dexlib2.iface.DexFile;
import com.android.tools.smali.dexlib2.iface.Method;
import com.android.tools.smali.dexlib2.iface.Field;
import com.android.tools.smali.dexlib2.iface.MethodImplementation;
import com.android.tools.smali.dexlib2.iface.instruction.Instruction;
import com.android.tools.smali.dexlib2.iface.instruction.ReferenceInstruction;
import com.android.tools.smali.dexlib2.iface.reference.StringReference;
import com.android.tools.smali.dexlib2.iface.reference.FieldReference;
import com.android.tools.smali.dexlib2.iface.reference.MethodReference;
import java.io.File;
import java.util.*;

/**
 * Dex analysis tools for patch development. <apk|dex> is an APK, a .dex file, or a directory of .dex files.
 *
 * Usage:
 *   java -cp <classpath> DexTools <command> <apk|dex> [args...]
 *
 * Commands:
 *   search-string <apk|dex> <string>           Search for methods containing a string
 *   search-strings <apk|dex> <s1> <s2> ...     Find methods containing ALL given strings
 *   dump-class <apk|dex> <class-type>          Dump all methods/fields of a class (e.g. LX/2fr;)
 *   dump-method <apk|dex> <class-type> <name>  Dump instructions of a specific method
 *   xref <apk|dex> <reference>                 Find cross-references to a class/method/field
 *   search-class <apk|dex> <query>             Search for classes by name (substring match)
 */
public class DexTools {

    public static void main(String[] args) throws Exception {
        if (args.length < 2) {
            System.out.println("Usage: DexTools <command> <apk|dex> [args...]");
            System.out.println("Commands: search-string, search-strings, dump-class, dump-method, xref, search-class");
            System.exit(1);
        }

        String command = args[0];
        String dexDir = args[1];

        switch (command) {
            case "search-string":
                if (args.length < 3) { System.out.println("Usage: search-string <apk|dex> <string>"); return; }
                searchString(dexDir, args[2]);
                break;
            case "search-strings":
                if (args.length < 4) { System.out.println("Usage: search-strings <apk|dex> <s1> <s2> ..."); return; }
                searchStrings(dexDir, Arrays.copyOfRange(args, 2, args.length));
                break;
            case "dump-class":
                if (args.length < 3) { System.out.println("Usage: dump-class <apk|dex> <class-type>"); return; }
                dumpClass(dexDir, args[2]);
                break;
            case "dump-method":
                if (args.length < 4) { System.out.println("Usage: dump-method <apk|dex> <class-type> <method-name>"); return; }
                dumpMethod(dexDir, args[2], args[3]);
                break;
            case "xref":
                if (args.length < 3) { System.out.println("Usage: xref <apk|dex> <reference>"); return; }
                xref(dexDir, args[2]);
                break;
            case "search-class":
                if (args.length < 3) { System.out.println("Usage: search-class <apk|dex> <query>"); return; }
                searchClass(dexDir, args[2]);
                break;
            default:
                System.out.println("Unknown command: " + command);
        }
    }

    private static List<DexFile> loadDexes(String path) throws Exception {
        File file = new File(path);
        File[] sources = file.isDirectory() ? file.listFiles((d, name) -> name.endsWith(".dex")) : new File[] { file };
        List<DexFile> dexes = new ArrayList<>();
        for (File source : sources) {
            var container = DexFileFactory.loadDexContainer(source, Opcodes.getDefault());
            for (String entry : container.getDexEntryNames()) dexes.add(container.getEntry(entry).getDexFile());
        }
        return dexes;
    }

    // Search for methods containing a specific string
    static void searchString(String dexDir, String searchString) throws Exception {
        for (DexFile dex : loadDexes(dexDir)) {
            for (ClassDef classDef : dex.getClasses()) {
                for (Method method : classDef.getMethods()) {
                    MethodImplementation impl = method.getImplementation();
                    if (impl == null) continue;
                    for (Instruction insn : impl.getInstructions()) {
                        if (insn instanceof ReferenceInstruction) {
                            var ref = ((ReferenceInstruction) insn).getReference();
                            if (ref instanceof StringReference) {
                                String str = ((StringReference) ref).getString();
                                if (str.contains(searchString)) {
                                    System.out.println("STRING: " + str);
                                    System.out.println("  CLASS: " + classDef.getType());
                                    System.out.println("  METHOD: " + method.getName() + "(" + method.getParameterTypes() + ")" + method.getReturnType());
                                    System.out.println("  ACCESS: " + method.getAccessFlags());
                                    System.out.println();
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // Find methods containing ALL given strings and dump their class + instructions
    static void searchStrings(String dexDir, String[] searchStrings) throws Exception {
        Set<String> required = new HashSet<>(Arrays.asList(searchStrings));
        for (DexFile dex : loadDexes(dexDir)) {
            for (ClassDef classDef : dex.getClasses()) {
                for (Method method : classDef.getMethods()) {
                    MethodImplementation impl = method.getImplementation();
                    if (impl == null) continue;

                    Set<String> found = new HashSet<>();
                    for (Instruction insn : impl.getInstructions()) {
                        if (insn instanceof ReferenceInstruction) {
                            var ref = ((ReferenceInstruction) insn).getReference();
                            if (ref instanceof StringReference) {
                                String str = ((StringReference) ref).getString();
                                for (String s : required) {
                                    if (str.equals(s)) found.add(s);
                                }
                            }
                        }
                    }

                    if (found.containsAll(required)) {
                        System.out.println("=== MATCH ===");
                        System.out.println("CLASS: " + classDef.getType());
                        System.out.println("METHOD: " + method.getName() + "(" + method.getParameterTypes() + ")" + method.getReturnType());
                        System.out.println("ACCESS: " + method.getAccessFlags());
                        System.out.println();

                        System.out.println("--- All methods in class ---");
                        for (Method m : classDef.getMethods()) {
                            System.out.println("  " + m.getName() + "(" + m.getParameterTypes() + ")" + m.getReturnType() + " access=" + m.getAccessFlags());
                        }

                        System.out.println();
                        System.out.println("--- Instructions ---");
                        int idx = 0;
                        for (Instruction insn : impl.getInstructions()) {
                            String extra = "";
                            if (insn instanceof ReferenceInstruction) {
                                extra = " -> " + ((ReferenceInstruction) insn).getReference();
                            }
                            System.out.println("  " + idx + ": " + insn.getOpcode() + extra);
                            idx++;
                        }
                        System.out.println();
                    }
                }
            }
        }
    }

    // Dump all methods and fields of a class
    static void dumpClass(String dexDir, String className) throws Exception {
        for (DexFile dex : loadDexes(dexDir)) {
            for (ClassDef classDef : dex.getClasses()) {
                if (classDef.getType().equals(className)) {
                    System.out.println("CLASS: " + classDef.getType());
                    System.out.println("SUPER: " + classDef.getSuperclass());
                    System.out.println("INTERFACES: " + classDef.getInterfaces());
                    System.out.println();
                    System.out.println("=== FIELDS ===");
                    for (Field f : classDef.getFields()) {
                        System.out.println("  " + f.getName() + " : " + f.getType() + " (access=" + f.getAccessFlags() + ")");
                    }
                    System.out.println();
                    System.out.println("=== METHODS ===");
                    for (Method method : classDef.getMethods()) {
                        System.out.println("  " + method.getName() + "(" + method.getParameterTypes() + ")" + method.getReturnType() + " access=" + method.getAccessFlags());
                        MethodImplementation impl = method.getImplementation();
                        if (impl == null) continue;
                        for (Instruction insn : impl.getInstructions()) {
                            if (insn instanceof ReferenceInstruction) {
                                var ref = ((ReferenceInstruction) insn).getReference();
                                if (ref instanceof StringReference) {
                                    System.out.println("    STR: " + ((StringReference) ref).getString());
                                }
                            }
                        }
                    }
                    return;
                }
            }
        }
        System.out.println("Class not found: " + className);
    }

    // Dump instructions of a specific method
    static void dumpMethod(String dexDir, String className, String methodName) throws Exception {
        for (DexFile dex : loadDexes(dexDir)) {
            for (ClassDef classDef : dex.getClasses()) {
                if (!classDef.getType().equals(className)) continue;
                for (Method method : classDef.getMethods()) {
                    if (!method.getName().equals(methodName)) continue;
                    System.out.println("METHOD: " + method.getName() + "(" + method.getParameterTypes() + ")" + method.getReturnType());
                    System.out.println("ACCESS: " + method.getAccessFlags());
                    MethodImplementation impl = method.getImplementation();
                    if (impl == null) { System.out.println("  NO IMPLEMENTATION"); continue; }
                    int idx = 0;
                    for (Instruction insn : impl.getInstructions()) {
                        String extra = "";
                        if (insn instanceof ReferenceInstruction) {
                            extra = " -> " + ((ReferenceInstruction) insn).getReference();
                        }
                        System.out.println("  " + idx + ": " + insn.getOpcode() + extra);
                        idx++;
                    }
                    System.out.println();
                }
            }
        }
    }

    // Find cross-references to a class, method, or field
    static void xref(String dexDir, String searchRef) throws Exception {
        for (DexFile dex : loadDexes(dexDir)) {
            for (ClassDef classDef : dex.getClasses()) {
                for (Method method : classDef.getMethods()) {
                    MethodImplementation impl = method.getImplementation();
                    if (impl == null) continue;
                    for (Instruction insn : impl.getInstructions()) {
                        if (insn instanceof ReferenceInstruction) {
                            var ref = ((ReferenceInstruction) insn).getReference();
                            String refStr = ref.toString();
                            if (refStr.contains(searchRef)) {
                                System.out.println("REF: " + refStr);
                                System.out.println("  IN CLASS: " + classDef.getType());
                                System.out.println("  IN METHOD: " + method.getName() + "(" + method.getParameterTypes() + ")" + method.getReturnType());
                                System.out.println("  ACCESS: " + method.getAccessFlags());
                                System.out.println();
                            }
                        }
                    }
                }
            }
        }
    }

    // Search for classes by name (substring match)
    static void searchClass(String dexDir, String query) throws Exception {
        for (DexFile dex : loadDexes(dexDir)) {
            for (ClassDef classDef : dex.getClasses()) {
                if (classDef.getType().toLowerCase().contains(query.toLowerCase())) {
                    System.out.println("CLASS: " + classDef.getType());
                    System.out.println("  SUPER: " + classDef.getSuperclass());
                    int methodCount = 0;
                    for (Method m : classDef.getMethods()) methodCount++;
                    int fieldCount = 0;
                    for (Field f : classDef.getFields()) fieldCount++;
                    System.out.println("  METHODS: " + methodCount + "  FIELDS: " + fieldCount);
                    System.out.println();
                }
            }
        }
    }
}
