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
import java.util.Date;
import java.util.HashSet;
import java.util.Map;
import java.util.Set;
import java.util.stream.Collectors;
import java.util.stream.Stream;

public class MoveFolder {
    public static void main(String args[]) throws IOException, ImageProcessingException, ParseException {
        String sourceFolderPath = "G:\\xena\\downloaded-photos";
//        String sourceFolderPath = "F:\\arranged-photos\\webp";
        String targetFolderPath = "G:\\xena\\arranged-photos";
        Set<String> fileNames = listFilesUsingFilesList(sourceFolderPath);
//        getFileTypes(fileNames);
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
//                System.out.println(tag.getTagName() + " - " + tag.getDescription());
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
    public static void moveToCorrespondingFolder(Set<String> fileNames, String sourceDir, String targetDir) throws IOException, ImageProcessingException, ParseException {
        Set<String> dateFolders = new HashSet<>();
//        int i = 0;
        for (String fileName : fileNames) {
//            if (i > 100) {
//                break;
//            }
            try {
                Path sourceFilePath = Paths.get(sourceDir + "\\" + fileName);
                String dateCreated = getDateFolder(sourceDir + "\\" + fileName);
                if (!dateFolders.contains(dateCreated)) {
                    // create new folder
                    File newDirectory = new File(targetDir + "\\" + dateCreated);
                    if (!newDirectory.exists()) {
                        newDirectory.mkdir();
                    }
                    // push
                    dateFolders.add(dateCreated);
                }
                // move file to folder
//            i++;

                Files.move(sourceFilePath, Paths.get(targetDir + "/" + dateCreated + "/" + fileName));
//            Files.copy(sourceFilePath, Paths.get(targetDir + "/" + dateCreated + "/" + fileName));
                System.out.println(String.format("Move file from [%s] to [%s]", sourceDir + "\\" + fileName, targetDir + "\\" + dateCreated + "\\" + fileName));
            } catch (Exception e) {

            }
        }
        System.out.println();
    }
}
