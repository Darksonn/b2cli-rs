# b2cli-rs

This is a commandline tool that allows uploading and downloading files to
backblaze B2.

This tool uses my library [backblaze-b2][1], and was primarely built to provide
a proof of concept of that library.

When uploading files it prints the url that the file is available on, and if
the bucket is public, the url should directly download the file.

# Usage

    b2cli [OPTIONS] --bucket <BUCKET>

## Flags

    -h, --help       Prints help information
    -V, --version    Prints version information

## Options

    -a, --auth <FILE>                        Specifies the json file containing the credentials for b2 [default: credentials.txt]
    -b, --bucket <BUCKET>                    Specify the b2 bucket to interact with
    -d, --download <B2FILE> <DESTINATION>    Each occurence of this action specifies a file to download
    -u, --upload <LOCAL> <DESTINATION>       Each occurence of this action specifies a file to upload

# Example

The example below uploads the files `files/file01` and `files/file02` as
`file01.txt` and `file02.txt` respectively, while also downloading `file03.txt`
as `files/file03`.

    b2cli -b bucket -u files/file01 file01.txt -u files/file02 file02.txt -d file03.txt files/file03

Note that this library currently prints a lot of information.

  [1]: https://github.com/Darksonn/backblaze-b2-rs
