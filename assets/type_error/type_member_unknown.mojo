# expect: 'Int32' value has no attribute 'size'
# A type name read as a value is its metatype, which has no attribute of that
# name: the pin reports the missing attribute, never an undefined name.


def main():
    print(Int32.size)
