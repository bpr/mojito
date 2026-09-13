# Mojito masks a shift amount to the word (`& 63`), so an over-wide shift has
# a defined answer; the pin cannot even fold one.
def main():
    print(UInt(1) << UInt(70), UInt(1024) >> UInt(68))
