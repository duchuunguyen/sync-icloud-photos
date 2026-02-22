package org.example;

import junit.framework.Test;
import junit.framework.TestCase;
import junit.framework.TestSuite;

/**
 * Unit tests for DuplicateCleaner.normalizeFileNameForDuplicateCheck and areDuplicateFilenames.
 */
public class DuplicateCleanerTest extends TestCase {

    public DuplicateCleanerTest(String testName) {
        super(testName);
    }

    public static Test suite() {
        return new TestSuite(DuplicateCleanerTest.class);
    }

    // --- normalizeFileNameForDuplicateCheck: pairs that should normalize to the same key ---

    public void testNormalize_sameKey_withParenNoSpace() {
        assertEquals("IMG_0381.jpg", DuplicateCleaner.normalizeFileNameForDuplicateCheck("IMG_0381(1).jpg"));
        assertEquals("IMG_0381.jpg", DuplicateCleaner.normalizeFileNameForDuplicateCheck("IMG_0381.jpg"));
        assertTrue(DuplicateCleaner.areDuplicateFilenames("IMG_0381(1).jpg", "IMG_0381.jpg"));
    }

    public void testNormalize_sameKey_withParenWithSpace() {
        assertEquals("IMG_0381.jpg", DuplicateCleaner.normalizeFileNameForDuplicateCheck("IMG_0381 (2).jpg"));
        assertTrue(DuplicateCleaner.areDuplicateFilenames("IMG_0381 (2).jpg", "IMG_0381.jpg"));
    }

    public void testNormalize_sameKey_videoExtension() {
        assertEquals("VID.mov", DuplicateCleaner.normalizeFileNameForDuplicateCheck("VID(1).mov"));
        assertEquals("VID.mov", DuplicateCleaner.normalizeFileNameForDuplicateCheck("VID.mov"));
        assertTrue(DuplicateCleaner.areDuplicateFilenames("VID(1).mov", "VID.mov"));
    }

    public void testNormalize_sameKey_multipleInGroup() {
        String key = DuplicateCleaner.normalizeFileNameForDuplicateCheck("IMG_0381(1).jpg");
        assertEquals(key, DuplicateCleaner.normalizeFileNameForDuplicateCheck("IMG_0381.jpg"));
        assertEquals(key, DuplicateCleaner.normalizeFileNameForDuplicateCheck("IMG_0381 (2).jpg"));
    }

    // --- pairs that should NOT be equal (different normalized keys) ---

    public void testNormalize_differentBase_differentKey() {
        assertFalse(DuplicateCleaner.areDuplicateFilenames("IMG_0381.jpg", "IMG_0382.jpg"));
        assertFalse(DuplicateCleaner.areDuplicateFilenames("IMG_0381(1).jpg", "IMG_0382(1).jpg"));
    }

    public void testNormalize_differentExtension_differentKey() {
        assertFalse(DuplicateCleaner.areDuplicateFilenames("IMG_0381.jpg", "IMG_0381.png"));
    }

    // --- edge cases ---

    public void testNormalize_noExtension() {
        assertEquals("file", DuplicateCleaner.normalizeFileNameForDuplicateCheck("file(1)"));
        assertEquals("file", DuplicateCleaner.normalizeFileNameForDuplicateCheck("file"));
    }

    public void testNormalize_doubleDigitInParen() {
        assertEquals("file.txt", DuplicateCleaner.normalizeFileNameForDuplicateCheck("file (10).txt"));
        assertEquals("file.txt", DuplicateCleaner.normalizeFileNameForDuplicateCheck("file.txt"));
    }

    public void testNormalize_nullReturnsNull() {
        assertNull(DuplicateCleaner.normalizeFileNameForDuplicateCheck(null));
    }

    public void testNormalize_emptyReturnsEmpty() {
        assertEquals("", DuplicateCleaner.normalizeFileNameForDuplicateCheck(""));
    }

    public void testAreDuplicateFilenames_null() {
        assertFalse(DuplicateCleaner.areDuplicateFilenames(null, "a.jpg"));
        assertFalse(DuplicateCleaner.areDuplicateFilenames("a.jpg", null));
        assertFalse(DuplicateCleaner.areDuplicateFilenames(null, null));
    }

    public void testNormalize_nameExactlyParenNumber_avoidStrippingWholeName() {
        // If the base is only "(1)", stripping would leave empty; plan says avoid stripping whole name
        String result = DuplicateCleaner.normalizeFileNameForDuplicateCheck("(1).jpg");
        assertEquals("(1).jpg", result);
    }
}
