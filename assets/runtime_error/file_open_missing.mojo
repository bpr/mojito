# expect: No such file or directory
# Opening a missing file for reading raises with glibc's `strerror` text.
def main() raises:
    var f = open(String("mojito_missing_dir/nothing.txt"), "r")
