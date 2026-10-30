# expect: type 'Int32' has no field 'size'
# A type name with no member of that name is not read as a value: the pin
# reports the missing attribute, never an undefined name.


def main():
    print(Int32.size)
