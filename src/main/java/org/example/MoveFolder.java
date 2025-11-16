package org.example;

import com.drew.imaging.ImageMetadataReader;
import com.drew.imaging.ImageProcessingException;
import com.drew.imaging.heif.HeifMetadataReader;
import com.drew.metadata.Directory;
import com.drew.metadata.Metadata;
import com.drew.metadata.Tag;

import java.io.File;
import java.io.FileInputStream;
import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.Paths;
import java.nio.file.attribute.BasicFileAttributes;
import java.nio.file.attribute.FileTime;
import java.text.ParseException;
import java.text.SimpleDateFormat;
import java.util.Collection;
import java.util.Date;
import java.util.HashSet;
import java.util.Map;
import java.util.Set;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicInteger;
import java.util.function.Consumer;
import java.util.stream.Collectors;
import java.util.stream.Stream;

public class MoveFolder {
    private static final int THREAD_POOL_SIZE = 20;
    private static final long SHUTDOWN_TIMEOUT_MINUTES = 10;
    public static void main(String args[]) throws IOException, ImageProcessingException, ParseException, InterruptedException {
        String sourceFolderPath = "G:\\xena\\downloaded-photos";
        String targetFolderPath = "G:\\xena\\arranged-photos";
        Set<String> fileNames = listFilesUsingFilesList(sourceFolderPath);
        moveToCorrespondingFolder(fileNames, sourceFolderPath, targetFolderPath);
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

    public static Set<String> getFileTypes(Set<String> fileNames) {
        Set<String> extensions = new HashSet<>();
        for (String fileName: fileNames) {
            extensions.add(com.google.common.io.Files.getFileExtension(fileName).toLowerCase());
        }
        System.out.println(extensions);
        return extensions;
    }
    public static String getDateFolder(String filePath) throws ImageProcessingException, IOException, ParseException {
        Set<String> imageTypes = Set.of("jpg", "heic", "png", "jpeg");
        Set<String> videoTypes = Set.of("mov", "mp4");
        String fileType = com.google.common.io.Files.getFileExtension(filePath).toLowerCase();
        if (!imageTypes.contains(fileType) && !videoTypes.contains(fileType)) {
            return "webp";
        }
        String tagName = "File Modified Date";
        if (imageTypes.contains(fileType)) {
            tagName = "Date/Time Original";
        }
        if (videoTypes.contains(fileType)) {
            tagName = "Creation Date";
        }
        Metadata metadata = ImageMetadataReader.readMetadata(new File(filePath));
        for (Directory directory : metadata.getDirectories()) {
            if (!directory.getTags().contains(tagName)) {
                tagName = "File Modified Date";
            }
            for (Tag tag : directory.getTags()) {
                if (tagName.equals(tag.getTagName())) {
                    String value = tag.getDescription();
                    // parse value
                    SimpleDateFormat df = new SimpleDateFormat("yyyy-MM");
                    switch (tagName) {
                        case "Date/Time Original":
                            df = new SimpleDateFormat("yyyy:MM:dd HH:mm:ss");
                            break;
                        case "Creation Date":
                            df = new SimpleDateFormat("yyyy-MM-dd'T'HH:mm:ssXXX");
                            break;
                        case "File Modified Date":
                            df = new SimpleDateFormat("EEE MMM dd HH:mm:ss XXX yyyy");
                            break;
                    }
                    Date createdDate = df.parse(value);
                    SimpleDateFormat folderFormat = new SimpleDateFormat("yyyy-MM");
                    return folderFormat.format(createdDate);
                }
            }
        }
        return "webp";
    }
    /**
     * Common function for executing tasks in parallel using a thread pool.
     * This ensures consistent parallel execution across all move operations.
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

    public static void moveToCorrespondingFolder(Set<String> fileNames, String sourceDir, String targetDir) throws IOException, ImageProcessingException, ParseException, InterruptedException {
        int totalFiles = fileNames.size();
        System.out.println("========================================");
        System.out.println("Starting parallel file move");
        System.out.println("Total files to move: " + totalFiles);
        System.out.println("========================================");

        // Thread-safe set for tracking created folders
        Set<String> dateFolders = java.util.Collections.synchronizedSet(new HashSet<>());

        // Thread-safe counter for tracking progress
        AtomicInteger movedCount = new AtomicInteger(0);
        // Start time for tracking elapsed and remaining time
        long startTime = System.currentTimeMillis();

        executeInParallel(fileNames, fileName -> {
            try {
                Path sourceFilePath = Paths.get(sourceDir + "\\" + fileName);
                String dateCreated = getDateFolder(sourceDir + "\\" + fileName);

                // Synchronize folder creation to avoid race conditions
                synchronized (dateFolders) {
                    if (!dateFolders.contains(dateCreated)) {
                        // create new folder
                        File newDirectory = new File(targetDir + "\\" + dateCreated);
                        if (!newDirectory.exists()) {
                            newDirectory.mkdir();
                        }
                        // push
                        dateFolders.add(dateCreated);
                    }
                }

                // move file to folder
                Files.move(sourceFilePath, Paths.get(targetDir + "/" + dateCreated + "/" + fileName));

                int currentCount = movedCount.incrementAndGet();

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

                    System.out.println(String.format("Progress: %d/%d files moved (%.1f%%) | Elapsed: %s | Remaining: %s - Moved: %s",
                        currentCount, totalFiles, percentage, formatDuration(elapsedTime), remainingTimeStr, fileName));
                } else {
                    System.out.println(String.format("Move file from [%s] to [%s]", sourceDir + "\\" + fileName, targetDir + "\\" + dateCreated + "\\" + fileName));
                }
            } catch (Exception e) {
                int currentCount = movedCount.incrementAndGet();
                System.out.println("Error when moving file: " + fileName + " - " + e.getMessage() + " (Progress: " + currentCount + "/" + totalFiles + ")");
            }
        });

        long totalTime = System.currentTimeMillis() - startTime;
        System.out.println("========================================");
        System.out.println("File move completed: " + movedCount.get() + "/" + totalFiles + " files moved in " + formatDuration(totalTime));
        System.out.println("========================================");
    }
}
