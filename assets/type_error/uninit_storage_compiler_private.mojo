# The inline uninit-storage primitive is reachable only from the bundled
# standard-library crossing module.
# expect: compiler-private storage
def main():
    var a = __UninitStorage[Int]()
