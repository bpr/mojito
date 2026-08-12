# The audited Mojo head rejects the legacy static allocation surface; user
# code allocates through std.memory.
# expect: static UnsafePointer allocation was removed
def main():
    var p = UnsafePointer[Int].alloc(1)
