//! Structs for reading a ZIP archive

use crc32::Crc32Reader;
use compression::CompressionMethod;
use spec;
use result::{ZipResult, ZipError};
use std::io;
use std::io::prelude::*;
use std::collections::HashMap;
use util;
use podio::{ReadPodExt, LittleEndian};
use types::ZipFileData;
use cp437::FromCp437;

/// Wrapper for reading the contents of a ZIP file.
///
/// ```
/// fn doit() -> zip::result::ZipResult<()>
/// {
///     use std::io::prelude::*;
///
///     // For demonstration purposes we read from an empty buffer.
///     // Normally a File object would be used.
///     let buf: &[u8] = &[0u8; 128];
///     let mut reader = std::io::Cursor::new(buf);
///
///     let mut zip = try!(zip::ZipArchive::new(reader));
///
///     for i in 0..zip.len()
///     {
///         let mut file = zip.by_index(i).unwrap();
///         println!("Filename: {}", file.name());
///         let first_byte = try!(file.bytes().next().unwrap());
///         println!("{}", first_byte);
///     }
///     Ok(())
/// }
///
/// println!("Result: {:?}", doit());
/// ```
pub struct ZipArchive<R: Read + io::Seek>
{
    reader: R,
    files: Vec<ZipFileData>,
    names_map: HashMap<String, usize>,
}

enum ZipFileReader<'a> {
    Stored(Crc32Reader<io::Take<&'a mut Read>>),
}

/// A struct for reading a zip file
pub struct ZipFile<'a> {
    data: &'a ZipFileData,
    reader: ZipFileReader<'a>,
}

fn unsupported_zip_error<T>(detail: &'static str) -> ZipResult<T>
{
    Err(ZipError::UnsupportedArchive(detail))
}

impl<R: Read+io::Seek> ZipArchive<R>
{
    /// Opens a Zip archive and parses the central directory
    pub fn new(mut reader: R) -> ZipResult<ZipArchive<R>> {
        let footer = try!(spec::CentralDirectoryEnd::find_and_parse(&mut reader));

        if footer.disk_number != footer.disk_with_central_directory { return unsupported_zip_error("Support for multi-disk files is not implemented") }

        let directory_start = footer.central_directory_offset as u64;
        let number_of_files = footer.number_of_files_on_this_disk as usize;

        let mut files = Vec::with_capacity(number_of_files);
        let mut names_map = HashMap::new();

        try!(reader.seek(io::SeekFrom::Start(directory_start)));
        for _ in 0 .. number_of_files
        {
            let file = try!(central_header_to_zip_file(&mut reader));
            names_map.insert(file.file_name.clone(), files.len());
            files.push(file);
        }

        Ok(ZipArchive { reader: reader, files: files, names_map: names_map })
    }

    /// Number of files contained in this zip.
    ///
    /// ```
    /// fn iter() {
    ///     let mut zip = zip::ZipArchive::new(std::io::Cursor::new(vec![])).unwrap();
    ///
    ///     for i in 0..zip.len() {
    ///         let mut file = zip.by_index(i).unwrap();
    ///         // Do something with file i
    ///     }
    /// }
    /// ```
    pub fn len(&self) -> usize
    {
        self.files.len()
    }

    /// Search for a file entry by name
    pub fn by_name<'a>(&'a mut self, name: &str) -> ZipResult<ZipFile<'a>>
    {
        let index = match self.names_map.get(name) {
            Some(index) => *index,
            None => { return Err(ZipError::FileNotFound); },
        };
        self.by_index(index)
    }

    /// Get a contained file by index
    pub fn by_index<'a>(&'a mut self, file_number: usize) -> ZipResult<ZipFile<'a>>
    {
        if file_number >= self.files.len() { return Err(ZipError::FileNotFound); }
        let ref data = self.files[file_number];
        let pos = data.data_start;

        if data.encrypted
        {
            return unsupported_zip_error("Encrypted files are not supported")
        }

        try!(self.reader.seek(io::SeekFrom::Start(pos)));
        let limit_reader = (self.reader.by_ref() as &mut Read).take(data.compressed_size);

        let reader = match data.compression_method
        {
            CompressionMethod::Stored =>
            {
                ZipFileReader::Stored(Crc32Reader::new(
                    limit_reader,
                    data.crc32))
            },
            _ => return unsupported_zip_error("Compression method not supported"),
        };
        Ok(ZipFile { reader: reader, data: data })
    }

    /// Unwrap and return the inner reader object
    ///
    /// The position of the reader is undefined.
    pub fn into_inner(self) -> R
    {
        self.reader
    }
}

fn central_header_to_zip_file<R: Read+io::Seek>(reader: &mut R) -> ZipResult<ZipFileData>
{
    // Parse central header
    let signature = try!(reader.read_u32::<LittleEndian>());
    if signature != spec::CENTRAL_DIRECTORY_HEADER_SIGNATURE
    {
        return Err(ZipError::InvalidArchive("Invalid Central Directory header"))
    }

    try!(reader.read_u16::<LittleEndian>());
    try!(reader.read_u16::<LittleEndian>());
    let flags = try!(reader.read_u16::<LittleEndian>());
    let encrypted = flags & 1 == 1;
    let is_utf8 = flags & (1 << 11) != 0;
    let compression_method = try!(reader.read_u16::<LittleEndian>());
    let last_mod_time = try!(reader.read_u16::<LittleEndian>());
    let last_mod_date = try!(reader.read_u16::<LittleEndian>());
    let crc32 = try!(reader.read_u32::<LittleEndian>());
    let compressed_size = try!(reader.read_u32::<LittleEndian>());
    let uncompressed_size = try!(reader.read_u32::<LittleEndian>());
    let file_name_length = try!(reader.read_u16::<LittleEndian>()) as usize;
    let extra_field_length = try!(reader.read_u16::<LittleEndian>()) as usize;
    let file_comment_length = try!(reader.read_u16::<LittleEndian>()) as usize;
    try!(reader.read_u16::<LittleEndian>());
    try!(reader.read_u16::<LittleEndian>());
    try!(reader.read_u32::<LittleEndian>());
    let offset = try!(reader.read_u32::<LittleEndian>()) as u64;
    let file_name_raw = try!(ReadPodExt::read_exact(reader, file_name_length));
    let extra_field = try!(ReadPodExt::read_exact(reader, extra_field_length));
    let file_comment_raw  = try!(ReadPodExt::read_exact(reader, file_comment_length));

    let file_name = match is_utf8
    {
        true => String::from_utf8_lossy(&*file_name_raw).into_owned(),
        false => file_name_raw.from_cp437(),
    };
    let file_comment = match is_utf8
    {
        true => String::from_utf8_lossy(&*file_comment_raw).into_owned(),
        false => file_comment_raw.from_cp437(),
    };

    // Remember end of central header
    let return_position = try!(reader.seek(io::SeekFrom::Current(0)));

    // Parse local header
    try!(reader.seek(io::SeekFrom::Start(offset)));
    let signature = try!(reader.read_u32::<LittleEndian>());
    if signature != spec::LOCAL_FILE_HEADER_SIGNATURE
    {
        return Err(ZipError::InvalidArchive("Invalid local file header"))
    }

    try!(reader.seek(io::SeekFrom::Current(22)));
    let file_name_length = try!(reader.read_u16::<LittleEndian>()) as u64;
    let extra_field_length = try!(reader.read_u16::<LittleEndian>()) as u64;
    let magic_and_header = 4 + 22 + 2 + 2;
    let data_start = offset + magic_and_header + file_name_length + extra_field_length;

    // Construct the result
    let mut result = ZipFileData
    {
        encrypted: encrypted,
        compression_method: CompressionMethod::from_u16(compression_method),
        last_modified_time: util::msdos_datetime_to_tm(last_mod_time, last_mod_date),
        crc32: crc32,
        compressed_size: compressed_size as u64,
        uncompressed_size: uncompressed_size as u64,
        file_name: file_name,
        file_comment: file_comment,
        header_start: offset,
        data_start: data_start,
    };

    try!(parse_extra_field(&mut result, &*extra_field));

    // Go back after the central header
    try!(reader.seek(io::SeekFrom::Start(return_position)));

    Ok(result)
}

fn parse_extra_field(_file: &mut ZipFileData, data: &[u8]) -> ZipResult<()>
{
    let mut reader = io::Cursor::new(data);

    while (reader.position() as usize) < data.len()
    {
        let kind = try!(reader.read_u16::<LittleEndian>());
        let len = try!(reader.read_u16::<LittleEndian>());
        match kind
        {
            _ => try!(reader.seek(io::SeekFrom::Current(len as i64))),
        };
    }
    Ok(())
}

/// Methods for retreiving information on zip files
impl<'a> ZipFile<'a> {
    fn get_reader(&mut self) -> &mut Read {
        match self.reader {
           ZipFileReader::Stored(ref mut r) => r as &mut Read,
        }
    }
    /// Get the name of the file
    pub fn name(&self) -> &str {
        &*self.data.file_name
    }
    /// Get the comment of the file
    pub fn comment(&self) -> &str {
        &*self.data.file_comment
    }
    /// Get the compression method used to store the file
    pub fn compression(&self) -> CompressionMethod {
        self.data.compression_method
    }
    /// Get the size of the file in the archive
    pub fn compressed_size(&self) -> u64 {
        self.data.compressed_size
    }
    /// Get the size of the file when uncompressed
    pub fn size(&self) -> u64 {
        self.data.uncompressed_size
    }
    /// Get the time the file was last modified
    pub fn last_modified(&self) -> ::time::Tm {
        self.data.last_modified_time
    }
}

impl<'a> Read for ZipFile<'a> {
     fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
         self.get_reader().read(buf)
     }
}
