# expect: Bad file descriptor
# Reading a handle opened for writing only raises with libc's `EBADF` text.
def main() raises:
    var f = open(String("/dev/null"), "w")
    print(f.read())
