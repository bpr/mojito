# split returns eager owned List[String] pieces; adjacent, leading, and
# trailing separators yield empty pieces, and a multibyte separator splits
# on its full byte sequence.
def main() raises:
    var parts = String("a,bb,,c").split(",")
    print(len(parts))
    for piece in parts:
        print(piece)
    var edged = String(",x,").split(",")
    print(len(edged))
    print(edged[1])
    var arrows = String("x→y→z").split("→")
    print(len(arrows))
    print(arrows[2])
    var whole = String("solo").split(",")
    print(len(whole))
    print(whole[0])
