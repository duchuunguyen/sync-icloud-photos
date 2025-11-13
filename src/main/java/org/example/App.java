package org.example;

import org.apache.commons.codec.digest.DigestUtils;

import java.io.IOException;
import java.io.InputStream;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.Paths;
import java.nio.file.StandardCopyOption;
import java.util.ArrayList;
import java.util.List;
import java.util.Set;
import java.util.concurrent.ForkJoinPool;
import java.util.stream.Collectors;
import java.util.stream.Stream;

import static java.lang.Thread.sleep;

public class App {
    public static void main(String[] args) throws IOException, InterruptedException {
        String sourceFolderPath = "D:\\xena\\iCloud Photos\\Photos";
        String targetFolderPath = "G:\\xena\\downloaded-photos";
        Set<String> fileNames = listFilesUsingFilesList(sourceFolderPath);
        System.out.println(String.join("\n", fileNames.toString()));

        ForkJoinPool customThreadPool = new ForkJoinPool(200);
        customThreadPool.submit(
                () -> fileNames.parallelStream()
                        .forEach(fileName -> {
                            try {
                                syncFile(fileName, sourceFolderPath, targetFolderPath);
                            } catch (IOException e) {
                                System.out.println("Error when syncing file: " + fileName);
                            }
                        }));
        while (!customThreadPool.isShutdown()) {
            sleep(1000);
        }
        sync2ways(sourceFolderPath, targetFolderPath);
        List<String> errorFiles = new ArrayList<>();
        for (String fileName : fileNames) {
            System.out.println("Checking file: " + fileName);
            boolean isIdentical = verify(fileName, sourceFolderPath, targetFolderPath);
            if (!isIdentical) {
                errorFiles.add(fileName);
            }
        }
        System.out.println("===================");
        for (String fileName : fileNames) {
            System.out.println(fileName + " file is not identical in source and target folder");
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

    public static void sync2ways(String sourceDir, String targetDir) throws IOException {
        Set<String> sourceFiles = listFilesUsingFilesList(sourceDir);
        Set<String> targetFiles = listFilesUsingFilesList(targetDir);
        System.out.println("=== Checking redundant files ===");
        targetFiles.parallelStream()
                .forEach(filename -> {
                    if (!sourceFiles.contains(filename)) {
                        Path targetPath = Paths.get(targetDir + "/" + filename);
                        try {
                            Files.delete(targetPath);
                        } catch (IOException e) {
                            System.out.println("Error when delete " + filename);
                        }
                    }
                });
        System.out.println("=== Checking missing files ===");
        sourceFiles.parallelStream()
                .forEach(filename -> {
                    if (!targetFiles.contains(filename)) {
                        System.out.println("Missing file in target folder: " + filename);
                    }
                });
    }

    public static void syncFile(String fileName, String sourceDir, String targetDir) throws IOException {
        Path copied = Paths.get(targetDir + "/" + fileName);
        Path originalPath = Paths.get(sourceDir + "/" + fileName);
        if (copied.toFile().exists()) {
            String originalChecksum = getChecksum(originalPath);
            String copiedChecksum = getChecksum(copied);
            if (originalChecksum.equals(copiedChecksum)) {
                return;
            }
        }
        System.out.println("Syncing: " + fileName);
        Files.copy(originalPath, copied, StandardCopyOption.REPLACE_EXISTING);
    }

    public static boolean verify(String fileName, String sourceDir, String targetDir) {
        Path copied = Paths.get(targetDir + "/" + fileName);
        Path originalPath = Paths.get(sourceDir + "/" + fileName);

        if (copied.toFile().exists()) {
            String originalChecksum = getChecksum(originalPath);
            String copiedChecksum = getChecksum(copied);
            return originalChecksum.equals(copiedChecksum);
        }
        return false;
    }
}
