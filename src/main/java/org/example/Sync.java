package org.example;

import org.apache.commons.codec.digest.DigestUtils;

import java.io.IOException;
import java.io.InputStream;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.Paths;
import java.nio.file.StandardCopyOption;
import java.util.ArrayList;
import java.util.Collection;
import java.util.HashSet;
import java.util.List;
import java.util.Set;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.ExecutionException;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicInteger;
import java.util.function.Consumer;
import java.util.stream.Collectors;
import java.util.stream.Stream;

public class Sync {
    private static final int THREAD_POOL_SIZE = 20;
    private static final long SHUTDOWN_TIMEOUT_MINUTES = 10;
    private static final String ERROR_FILES_FILENAME = "error_files.txt";

    public static void main(String[] args) throws IOException, InterruptedException, ExecutionException {
        String sourceFolderPath = "D:\\xena\\iCloud Photos\\Photos";
        String targetFolderPath = "G:\\xena\\downloaded-photos";
        boolean isErrorFiles = true;
        syncErrorFiles(sourceFolderPath, targetFolderPath);
        Set<String> fileNames = isErrorFiles ? readErrorFiles() : listFilesUsingFilesList(sourceFolderPath);

        // Sync all files in parallel
        syncAllFiles(fileNames, sourceFolderPath, targetFolderPath);
            
        // Perform two-way sync check
        // sync2ways(sourceFolderPath, targetFolderPath);
            
        // Verify all files in parallel
        List<String> errorFiles = verifyAllFiles(fileNames, sourceFolderPath, targetFolderPath);

        // Write error files to file for later use
        if (!errorFiles.isEmpty()) {
            writeErrorFilesToFile(errorFiles);
            System.out.println("\nError files have been written to: " + Paths.get(System.getProperty("user.dir"), ERROR_FILES_FILENAME));
            System.out.println("You can use handleErrorFilesFromFile() later to process them.");
        }
        handleErrorFilesFromFile(targetFolderPath);
    }

    public static Set<String> readErrorFiles() throws IOException {
         // Read error files from file
        List<String> errorFilesList = readErrorFilesFromFile();
        
        if (errorFilesList.isEmpty()) {
            System.out.println("No error files found to sync. The error files list is empty or doesn't exist.");
            return new HashSet<>();
        }
        
        // Convert to Set to match syncAllFiles signature
        return new HashSet<>(errorFilesList);
    }

    /**
     * Common function for executing tasks in parallel using a thread pool.
     * This ensures consistent parallel execution across all sync and verify operations.
     *
     * @param items Collection of items to process
     * @param task Consumer that processes each item
     * @param <T> Type of items to process
     * @throws InterruptedException if interrupted while waiting
     */
    public static <T> void executeInParallel(Collection<T> items, Consumer<T> task) throws InterruptedException {
        if (items.isEmpty()) {
            return;
        }

        ExecutorService executor = Executors.newFixedThreadPool(THREAD_POOL_SIZE);
        CountDownLatch latch = new CountDownLatch(items.size());

        try {
            // Submit each item as a separate task to the thread pool
            for (T item : items) {
                executor.submit(() -> {
                    try {
                        task.accept(item);
                    } catch (Exception e) {
                        System.out.println("Error processing item: " + item + " - " + e.getMessage());
                    } finally {
                        latch.countDown(); // Signal completion
                    }
                });
            }

            // Wait for all tasks to complete
            latch.await();
        } finally {
            // Properly shutdown the thread pool to release resources
            shutdownExecutor(executor);
        }
    }

    /**
     * Shuts down the executor service gracefully.
     *
     * @param executor The executor service to shutdown
     */
    private static void shutdownExecutor(ExecutorService executor) {
        executor.shutdown();
        try {
            // Wait up to configured timeout for any remaining tasks
            if (!executor.awaitTermination(SHUTDOWN_TIMEOUT_MINUTES, TimeUnit.MINUTES)) {
                // Force shutdown if tasks are still running
                executor.shutdownNow();
            }
        } catch (InterruptedException e) {
            executor.shutdownNow();
        }
    }
    /**
     * Formats time duration in milliseconds to a human-readable string.
     *
     * @param milliseconds Time duration in milliseconds
     * @return Formatted string (e.g., "1h 23m 45s" or "45s")
     */
    private static String formatDuration(long milliseconds) {
        if (milliseconds < 0) {
            return "0s";
        }
        
        long seconds = milliseconds / 1000;
        long minutes = seconds / 60;
        long hours = minutes / 60;
        
        seconds = seconds % 60;
        minutes = minutes % 60;
        
        if (hours > 0) {
            return String.format("%dh %dm %ds", hours, minutes, seconds);
        } else if (minutes > 0) {
            return String.format("%dm %ds", minutes, seconds);
        } else {
            return String.format("%ds", seconds);
        }
    }

    /**
     * Syncs all files from source to target folder in parallel.
     *
     * @param fileNames Set of file names to sync
     * @param sourceDir Source directory path
     * @param targetDir Target directory path
     * @throws InterruptedException if interrupted while waiting
     */
    public static void syncAllFiles(Set<String> fileNames, String sourceDir, String targetDir) throws InterruptedException {
        int totalFiles = fileNames.size();
        System.out.println("========================================");
        System.out.println("Starting parallel file sync");
        System.out.println("Total files to sync: " + totalFiles);
        System.out.println("========================================");
        
        // Thread-safe counter for tracking progress
        AtomicInteger syncedCount = new AtomicInteger(0);
        // Start time for tracking elapsed and remaining time
        long startTime = System.currentTimeMillis();
        
        executeInParallel(fileNames, fileName -> {
            try {
                syncFile(fileName, sourceDir, targetDir);
                int currentCount = syncedCount.incrementAndGet();
                
                // Log progress every 10 files or on the last file
                if (currentCount % 10 == 0 || currentCount == totalFiles) {
                    long elapsedTime = System.currentTimeMillis() - startTime;
                    double percentage = (currentCount * 100.0) / totalFiles;
                    
                    // Calculate estimated remaining time
                    String remainingTimeStr = "calculating...";
                    if (currentCount > 0) {
                        long avgTimePerFile = elapsedTime / currentCount;
                        int remainingFiles = totalFiles - currentCount;
                        long estimatedRemaining = avgTimePerFile * remainingFiles;
                        remainingTimeStr = formatDuration(estimatedRemaining);
                    }
                    
                    System.out.println(String.format("Progress: %d/%d files synced (%.1f%%) | Elapsed: %s | Remaining: %s - Synced: %s", 
                        currentCount, totalFiles, percentage, formatDuration(elapsedTime), remainingTimeStr, fileName));
                } else {
                    System.out.println("Processing " + fileName);
                }
            } catch (IOException e) {
                int currentCount = syncedCount.incrementAndGet();
                System.out.println("Error when syncing file: " + fileName + " (Progress: " + currentCount + "/" + totalFiles + ")");
            }
        });
        
        long totalTime = System.currentTimeMillis() - startTime;
        System.out.println("========================================");
        System.out.println("File sync completed: " + syncedCount.get() + "/" + totalFiles + " files synced in " + formatDuration(totalTime));
        System.out.println("Starting two-way sync check...");
    }

    /**
     * Syncs only the files listed in the error files list from the current working directory.
     * This is useful for re-syncing files that failed verification after they have been deleted.
     *
     * @param sourceDir Source directory path
     * @param targetDir Target directory path
     * @throws IOException if file reading fails
     * @throws InterruptedException if interrupted while waiting
     */
    public static void syncErrorFiles(String sourceDir, String targetDir) throws IOException, InterruptedException {
        // Read error files from file
        List<String> errorFilesList = readErrorFilesFromFile();
        
        if (errorFilesList.isEmpty()) {
            System.out.println("No error files found to sync. The error files list is empty or doesn't exist.");
            return;
        }
        
        // Convert to Set to match syncAllFiles signature
        Set<String> errorFiles = new HashSet<>(errorFilesList);
        
        System.out.println("========================================");
        System.out.println("Syncing error files only");
        System.out.println("Found " + errorFiles.size() + " files in error list to sync");
        System.out.println("========================================");
        
        // Use syncAllFiles logic but with error files only
        syncAllFiles(errorFiles, sourceDir, targetDir);
        
        System.out.println("Error files sync completed.");
    }

    /**
     * Performs two-way sync: removes files from target that don't exist in source,
     * and identifies files missing in target. Both operations run in parallel.
     *
     * @param sourceDir Source directory path
     * @param targetDir Target directory path
     * @throws IOException if file listing fails
     * @throws InterruptedException if interrupted while waiting
     */
    public static void sync2ways(String sourceDir, String targetDir) throws IOException, InterruptedException {
        Set<String> sourceFiles = listFilesUsingFilesList(sourceDir);
        Set<String> targetFiles = listFilesUsingFilesList(targetDir);
        
        // Remove redundant files from target (files that don't exist in source)
        System.out.println("=== Checking redundant files ===");
        List<String> redundantFiles = targetFiles.stream()
                .filter(filename -> !sourceFiles.contains(filename))
                .collect(Collectors.toList());
        
        if (!redundantFiles.isEmpty()) {
            executeInParallel(redundantFiles, filename -> {
                Path targetPath = Paths.get(targetDir, filename);
                try {
                    Files.delete(targetPath);
                    System.out.println("Deleted redundant file: " + filename);
                } catch (IOException e) {
                    System.out.println("Error when deleting " + filename + ": " + e.getMessage());
                }
            });
        }
        
        // Identify missing files in target
        System.out.println("=== Checking missing files ===");
        List<String> missingFiles = sourceFiles.stream()
                .filter(filename -> !targetFiles.contains(filename))
                .collect(Collectors.toList());
        
        if (!missingFiles.isEmpty()) {
            executeInParallel(missingFiles, filename -> {
                System.out.println("Missing file in target folder: " + filename);
            });
        }
    }

    /**
     * Verifies all files by comparing checksums between source and target folders in parallel.
     *
     * @param fileNames Set of file names to verify
     * @param sourceDir Source directory path
     * @param targetDir Target directory path
     * @return List of file names that failed verification
     * @throws InterruptedException if interrupted while waiting
     */
    public static List<String> verifyAllFiles(Set<String> fileNames, String sourceDir, String targetDir) throws InterruptedException {
        int totalFiles = fileNames.size();
        System.out.println("========================================");
        System.out.println("Starting parallel file verification");
        System.out.println("Total files to check: " + totalFiles);
        System.out.println("========================================");
        
        List<String> errorFiles = new ArrayList<>();
        // Use synchronized list to safely add from multiple threads
        List<String> synchronizedErrorFiles = java.util.Collections.synchronizedList(errorFiles);
        
        // Thread-safe counter for tracking progress
        AtomicInteger checkedCount = new AtomicInteger(0);
        // Start time for tracking elapsed and remaining time
        long startTime = System.currentTimeMillis();
        
        executeInParallel(fileNames, fileName -> {
            boolean isIdentical = verify(fileName, sourceDir, targetDir);
            int currentCount = checkedCount.incrementAndGet();
            
            // Log progress every 10 files or on the last file
            if (currentCount % 10 == 0 || currentCount == totalFiles) {
                long elapsedTime = System.currentTimeMillis() - startTime;
                double percentage = (currentCount * 100.0) / totalFiles;
                
                // Calculate estimated remaining time
                String remainingTimeStr = "calculating...";
                if (currentCount > 0) {
                    long avgTimePerFile = elapsedTime / currentCount;
                    int remainingFiles = totalFiles - currentCount;
                    long estimatedRemaining = avgTimePerFile * remainingFiles;
                    remainingTimeStr = formatDuration(estimatedRemaining);
                }
                
                System.out.println(String.format("Progress: %d/%d files checked (%.1f%%) | Elapsed: %s | Remaining: %s - Checking: %s", 
                    currentCount, totalFiles, percentage, formatDuration(elapsedTime), remainingTimeStr, fileName));
            }
            
            if (!isIdentical) {
                synchronizedErrorFiles.add(fileName);
            }
        });
        
        long totalTime = System.currentTimeMillis() - startTime;
        System.out.println("========================================");
        System.out.println("Verification completed: " + checkedCount.get() + "/" + totalFiles + " files checked in " + formatDuration(totalTime));
        System.out.println("========================================");
        if (errorFiles.isEmpty()) {
            System.out.println("All files are identical in source and target folder");
        } else {
            System.out.println("Found " + errorFiles.size() + " files that are not identical:");
            for (String fileName : errorFiles) {
                System.out.println("  - " + fileName);
            }
        }
        
        return errorFiles;
    }

    /**
     * Writes error files list to a file in the current working directory for later use.
     *
     * @param errorFiles List of file names that failed verification
     * @throws IOException if file writing fails
     */
    public static void writeErrorFilesToFile(List<String> errorFiles) throws IOException {
        if (errorFiles.isEmpty()) {
            return;
        }
        
        // Write to current working directory (where the code is running)
        Path errorFilesPath = Paths.get(System.getProperty("user.dir"), ERROR_FILES_FILENAME);
        String content = String.join("\n", errorFiles);
        Files.write(errorFilesPath, content.getBytes(StandardCharsets.UTF_8));
        System.out.println("Written " + errorFiles.size() + " error file names to: " + errorFilesPath);
    }

    /**
     * Reads error files list from a file in the current working directory.
     *
     * @return List of file names that failed verification, or empty list if file doesn't exist
     * @throws IOException if file reading fails
     */
    public static List<String> readErrorFilesFromFile() throws IOException {
        // Read from current working directory (where the code is running)
        Path errorFilesPath = Paths.get(System.getProperty("user.dir"), ERROR_FILES_FILENAME);
        
        if (!Files.exists(errorFilesPath)) {
            System.out.println("Error files list not found: " + errorFilesPath);
            return new ArrayList<>();
        }
        
        List<String> errorFiles = Files.readAllLines(errorFilesPath, StandardCharsets.UTF_8);
        // Remove empty lines
        errorFiles.removeIf(String::isEmpty);
        System.out.println("Read " + errorFiles.size() + " error file names from: " + errorFilesPath);
        return errorFiles;
    }

    /**
     * Handles error files by deleting them from the target folder.
     *
     * @param errorFiles List of file names that failed verification
     * @param targetDir Target directory path
     */
    public static void handleErrorFiles(List<String> errorFiles, String targetDir) {
        if (errorFiles.isEmpty()) {
            return;
        }
        
        // Delete error files from target folder so they can be re-synced
        System.out.println("\nDeleting error files from target folder...");
        int deletedCount = 0;
        int failedCount = 0;
        for (String fileName : errorFiles) {
            Path targetFilePath = Paths.get(targetDir, fileName);
            try {
                if (Files.exists(targetFilePath)) {
                    Files.delete(targetFilePath);
                    System.out.println("Deleted: " + fileName);
                    deletedCount++;
                } else {
                    System.out.println("File not found in target (already deleted?): " + fileName);
                }
            } catch (IOException e) {
                System.out.println("Failed to delete " + fileName + ": " + e.getMessage());
                failedCount++;
            }
        }
        System.out.println("\nDeletion summary: " + deletedCount + " deleted, " + failedCount + " failed");
        System.out.println("You can now run sync again to re-sync these files.");
    }

    /**
     * Reads error files from file in current working directory and handles them by deleting from target folder.
     * This is a convenience method that combines readErrorFilesFromFile and handleErrorFiles.
     *
     * @param targetDir Target directory path where error files should be deleted from
     * @throws IOException if file reading fails
     */
    public static void handleErrorFilesFromFile(String targetDir) throws IOException {
        List<String> errorFiles = readErrorFilesFromFile();
        if (!errorFiles.isEmpty()) {
            handleErrorFiles(errorFiles, targetDir);
        } else {
            System.out.println("No error files to handle.");
        }
    }

    public static Set<String> listFilesUsingFilesList(String dir) throws IOException {
        try (Stream<Path> stream = Files.list(Paths.get(dir))) {
            return stream
                    .filter(file -> !Files.isDirectory(file))
                    .map(Path::getFileName)
                    .map(Path::toString)
                    .collect(Collectors.toSet());
        }
    }

    public static String getChecksum(Path filePath) {
        try (InputStream is = Files.newInputStream(filePath)) {
            return DigestUtils.md5Hex(is);
        } catch (IOException e) {
            throw new RuntimeException(e);
        }
    }

    public static void syncFile(String fileName, String sourceDir, String targetDir) throws IOException {
        Path copied = Paths.get(targetDir, fileName);
        Path originalPath = Paths.get(sourceDir, fileName);
        System.out.println("Checking " + fileName + " to " + targetDir + "/" + fileName);
        if (copied.toFile().exists()) {
           String originalChecksum = getChecksum(originalPath);
           String copiedChecksum = getChecksum(copied);
           if (originalChecksum.equals(copiedChecksum)) {
               System.out.println("File is identical in source and target folder: " + fileName);
               return;
           }
           System.out.println("File is not identical in source and target folder: " + fileName);
        }
        System.out.println("Syncing: " + fileName);
        Files.copy(originalPath, copied, StandardCopyOption.REPLACE_EXISTING);
        System.out.println("Synced: " + fileName);
    }

    public static boolean verify(String fileName, String sourceDir, String targetDir) {
        Path copied = Paths.get(targetDir, fileName);
        Path originalPath = Paths.get(sourceDir, fileName);

        if (copied.toFile().exists()) {
            String originalChecksum = getChecksum(originalPath);
            String copiedChecksum = getChecksum(copied);
            return originalChecksum.equals(copiedChecksum);
        }
        return false;
    }
}
