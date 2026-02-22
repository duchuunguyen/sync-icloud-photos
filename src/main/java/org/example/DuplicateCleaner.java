package org.example;

import com.google.gson.Gson;
import com.google.gson.GsonBuilder;

import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.Paths;
import java.util.ArrayList;
import java.util.Comparator;
import java.util.HashSet;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.Set;
import java.util.regex.Pattern;
import java.util.stream.Collectors;
import java.util.stream.Stream;

/**
 * Scans a root folder and its subfolders for duplicate files (by normalized filename
 * and by checksum), lists them, then removes duplicates keeping one per set.
 * Supports --dry-run (no deletion) and --output &lt;file&gt; to write a JSON report.
 */
public class DuplicateCleaner {

    private static final String DEFAULT_ROOT = "G:\\xena\\arranged-photos";
    private static final String DEFAULT_OUTPUT_JSON = "duplicates-report.json";

    /** Trailing " (N)" or "(N)" before extension (N = digits). */
    private static final Pattern TRAILING_PAREN_NUMBER = Pattern.compile("\\s*\\(\\d+\\)$");

    private static final Set<String> IMAGE_EXTENSIONS = Set.of("jpg", "jpeg", "heic", "png", "webp");
    private static final Set<String> VIDEO_EXTENSIONS = Set.of("mov", "mp4");
    private static final Set<String> MEDIA_EXTENSIONS = new HashSet<>();
    static {
        MEDIA_EXTENSIONS.addAll(IMAGE_EXTENSIONS);
        MEDIA_EXTENSIONS.addAll(VIDEO_EXTENSIONS);
    }

    public static void main(String[] args) throws IOException {
        String rootPath = DEFAULT_ROOT;
        boolean dryRun = false;
        Path outputJsonPath = null;

        for (int i = 0; i < args.length; i++) {
            if ("--dry-run".equals(args[i])) {
                dryRun = true;
            } else if ("--output".equals(args[i]) && i + 1 < args.length) {
                outputJsonPath = Paths.get(args[++i]);
            } else if (!args[i].startsWith("--")) {
                rootPath = args[i];
            }
        }

        Path root = Paths.get(rootPath);
        if (!Files.isDirectory(root)) {
            System.err.println("Not a directory: " + rootPath);
            return;
        }
        if (dryRun) {
            System.out.println("Dry run mode: no files will be deleted.");
        }
        run(root, dryRun, outputJsonPath);
    }

    /**
     * Normalizes a filename for duplicate detection by removing a trailing " (N)" or "(N)"
     * (N = digits) before the extension. Two filenames are considered the same if they
     * have the same normalized key.
     *
     * @param fileName file name (e.g. "IMG_0381(1).jpg")
     * @return normalized key (e.g. "IMG_0381.jpg"), or original if stripping would leave empty base
     */
    public static String normalizeFileNameForDuplicateCheck(String fileName) {
        if (fileName == null || fileName.isEmpty()) {
            return fileName;
        }
        int lastDot = fileName.lastIndexOf('.');
        String base;
        String ext;
        if (lastDot <= 0) {
            base = fileName;
            ext = "";
        } else {
            base = fileName.substring(0, lastDot);
            ext = fileName.substring(lastDot);
        }
        String stripped = TRAILING_PAREN_NUMBER.matcher(base).replaceFirst("");
        if (stripped.isEmpty()) {
            return fileName;
        }
        return stripped + ext;
    }

    /**
     * Returns true if the two filenames are considered the same for duplicate detection
     * (same normalized key).
     */
    public static boolean areDuplicateFilenames(String a, String b) {
        if (a == null || b == null) return false;
        return normalizeFileNameForDuplicateCheck(a).equals(normalizeFileNameForDuplicateCheck(b));
    }

    private static boolean isMediaFile(String fileName) {
        String ext = com.google.common.io.Files.getFileExtension(fileName).toLowerCase();
        return MEDIA_EXTENSIONS.contains(ext);
    }

    /** Picks the file to keep from a name-duplicate group: no "(N)" preferred, else smallest N, else first by name. */
    private static String chooseNameDuplicateToKeep(List<String> fileNames) {
        return fileNames.stream().min(Comparator
                .comparing(DuplicateCleaner::hasParenNumber)
                .thenComparing(DuplicateCleaner::extractParenNumber)
                .thenComparing(String::compareTo))
                .orElse(fileNames.get(0));
    }

    private static boolean hasParenNumber(String fileName) {
        return extractParenNumber(fileName) != Integer.MAX_VALUE;
    }

    /** Returns the number in trailing " (N)" or "(N)" or Integer.MAX_VALUE if none. */
    private static int extractParenNumber(String fileName) {
        int lastDot = fileName.lastIndexOf('.');
        String base = lastDot > 0 ? fileName.substring(0, lastDot) : fileName;
        java.util.regex.Matcher m = java.util.regex.Pattern.compile("\\s*\\((\\d+)\\)\\s*$").matcher(base);
        if (m.find()) {
            try {
                return Integer.parseInt(m.group(1));
            } catch (NumberFormatException e) {
                return Integer.MAX_VALUE;
            }
        }
        return Integer.MAX_VALUE;
    }

    public static void run(Path root) throws IOException {
        run(root, false, null);
    }

    /**
     * @param outputJsonPath if non-null, write a JSON report of all duplicates to this file
     */
    public static void run(Path root, boolean dryRun, Path outputJsonPath) throws IOException {
        List<Path> folders = collectFoldersToScan(root);
        System.out.println("Scanning " + folders.size() + " folder(s) under " + root);

        List<Map<String, Object>> folderReports = new ArrayList<>();
        Set<Path> allToDelete = new HashSet<>();

        for (Path folder : folders) {
            FolderResult result = processFolder(folder, folderReports, allToDelete);
            if (result != null) {
                printFolderResult(folder, result, dryRun);
            }
        }

        if (outputJsonPath != null) {
            Map<String, Object> report = new LinkedHashMap<>();
            report.put("root", root.toString());
            report.put("dryRun", dryRun);
            report.put("folders", folderReports);
            Gson gson = new GsonBuilder().setPrettyPrinting().create();
            Files.write(outputJsonPath, gson.toJson(report).getBytes(StandardCharsets.UTF_8));
            System.out.println("\nReport written to: " + outputJsonPath.toAbsolutePath());
        }

        if (!dryRun && !allToDelete.isEmpty()) {
            for (Path path : allToDelete) {
                try {
                    if (Files.exists(path)) {
                        Files.delete(path);
                        System.out.println("Deleted: " + path.getFileName());
                    }
                } catch (IOException e) {
                    System.err.println("Failed to delete " + path + ": " + e.getMessage());
                }
            }
        } else if (dryRun && !allToDelete.isEmpty()) {
            System.out.println("\nWould delete " + allToDelete.size() + " file(s).");
        }
    }

    private static void printFolderResult(Path folder, FolderResult result, boolean dryRun) {
        System.out.println("\n--- Folder: " + folder + " ---");
        if (!result.nameDuplicateGroups.isEmpty()) {
            System.out.println("Name duplicates (same checksum removed):");
            for (NameDuplicateGroup g : result.nameDuplicateGroups) {
                System.out.println("  Keep: " + g.keep + " | Remove: " + String.join(", ", g.remove));
            }
        }
    }

    private static class FolderResult {
        final List<NameDuplicateGroup> nameDuplicateGroups = new ArrayList<>();
    }

    private static class NameDuplicateGroup {
        final String keep;
        final List<String> remove;

        NameDuplicateGroup(String keep, List<String> remove) {
            this.keep = keep;
            this.remove = remove;
        }
    }

    private static List<Path> collectFoldersToScan(Path root) throws IOException {
        List<Path> folders = new ArrayList<>();
        folders.add(root);
        try (Stream<Path> stream = Files.walk(root, Integer.MAX_VALUE)) {
            stream.filter(p -> !p.equals(root))
                    .filter(Files::isDirectory)
                    .forEach(folders::add);
        }
        return folders;
    }

    /**
     * Processes one folder: finds name and checksum duplicates, fills folderReport for JSON,
     * adds paths to allToDelete. Does not delete anything.
     * @return FolderResult for console output, or null if no duplicates
     */
    private static FolderResult processFolder(Path folder,
                                             List<Map<String, Object>> folderReports,
                                             Set<Path> allToDelete) throws IOException {
        Set<String> fileNames = Sync.listFilesUsingFilesList(folder.toString());
        List<String> mediaFiles = fileNames.stream()
                .filter(DuplicateCleaner::isMediaFile)
                .sorted()
                .collect(Collectors.toList());
        if (mediaFiles.isEmpty()) return null;

        // Name-based duplicate groups (same normalized name)
        Map<String, List<String>> byNormalizedName = new LinkedHashMap<>();
        for (String name : mediaFiles) {
            String key = normalizeFileNameForDuplicateCheck(name);
            byNormalizedName.computeIfAbsent(key, k -> new ArrayList<>()).add(name);
        }
        List<List<String>> nameDuplicateGroups = byNormalizedName.values().stream()
                .filter(list -> list.size() > 1)
                .collect(Collectors.toList());

        if (nameDuplicateGroups.isEmpty()) {
            return null;
        }

        // Only checksum files that are in a name-duplicate group
        Map<String, String> fileNameToChecksum = new LinkedHashMap<>();
        for (List<String> nameGroup : nameDuplicateGroups) {
            for (String name : nameGroup) {
                if (fileNameToChecksum.containsKey(name)) continue;
                Path path = folder.resolve(name);
                if (!Files.isRegularFile(path)) continue;
                try {
                    fileNameToChecksum.put(name, Sync.getChecksum(path));
                } catch (Exception e) {
                    System.err.println("Checksum failed for " + path + ": " + e.getMessage());
                }
            }
        }

        FolderResult result = new FolderResult();
        Set<Path> toDelete = new HashSet<>();
        Map<String, Object> jsonFolder = new LinkedHashMap<>();
        jsonFolder.put("folderPath", folder.toString());
        List<Map<String, Object>> jsonNameDups = new ArrayList<>();

        // Within each name-duplicate group: only remove files that share a checksum (keep one per checksum)
        for (List<String> nameGroup : nameDuplicateGroups) {
            Map<String, List<String>> byChecksumInGroup = new LinkedHashMap<>();
            for (String name : nameGroup) {
                String cs = fileNameToChecksum.get(name);
                if (cs != null) {
                    byChecksumInGroup.computeIfAbsent(cs, k -> new ArrayList<>()).add(name);
                }
            }
            for (List<String> subgroup : byChecksumInGroup.values()) {
                if (subgroup.size() <= 1) continue;
                String keep = chooseNameDuplicateToKeep(subgroup);
                List<String> remove = subgroup.stream().filter(n -> !n.equals(keep)).collect(Collectors.toList());
                result.nameDuplicateGroups.add(new NameDuplicateGroup(keep, remove));
                Map<String, Object> entry = new LinkedHashMap<>();
                entry.put("keep", keep);
                entry.put("remove", remove);
                jsonNameDups.add(entry);
                for (String name : remove) {
                    toDelete.add(folder.resolve(name));
                }
            }
        }

        jsonFolder.put("nameDuplicates", jsonNameDups);
        folderReports.add(jsonFolder);
        allToDelete.addAll(toDelete);

        return result;
    }
}
