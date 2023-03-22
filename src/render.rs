use std::io::Cursor;
use std::num::NonZeroUsize;
use std::ops::Range;
use std::sync::Arc;

use typst::diag::SourceError;
use typst::geom::{Axis, Color, Size};
use typst::syntax::{ErrorPos, Source};

use crate::sandbox::Sandbox;
use crate::FILE_NAME;

const DESIRED_RESOLUTION: f32 = 1000.0;
const MAX_SIZE: f32 = 1000.0;

#[derive(Debug, thiserror::Error)]
#[error(
	"rendered output was too big: the {axis:?} axis was {size} pt but the maximum is {MAX_SIZE}"
)]
pub struct TooBig {
	size: f32,
	axis: Axis,
}

fn determine_pixels_per_point(size: Size) -> Result<f32, TooBig> {
	// We want to truncate.
	#![allow(clippy::cast_possible_truncation)]

	let x = size.x.to_pt() as f32;
	let y = size.y.to_pt() as f32;

	if x > MAX_SIZE {
		Err(TooBig {
			size: x,
			axis: Axis::X,
		})
	} else if y > MAX_SIZE {
		Err(TooBig {
			size: y,
			axis: Axis::Y,
		})
	} else {
		let area = x * y;
		Ok(DESIRED_RESOLUTION / area.sqrt())
	}
}

#[derive(Debug)]
pub struct SourceErrorsWithSource {
	source: Source,
	errors: Vec<SourceError>,
}

#[derive(Debug, Clone, Copy)]
struct CharIndex {
	first_byte: usize,
	char_index: usize,
}

impl std::ops::Add for CharIndex {
	type Output = CharIndex;

	fn add(self, other: Self) -> Self {
		Self {
			first_byte: self.first_byte + other.first_byte,
			char_index: self.char_index + other.char_index,
		}
	}
}

fn byte_index_to_char_index(source: &str, byte_index: usize) -> Option<CharIndex> {
	source
		.char_indices()
		.enumerate()
		.map(|(char_index, (first_byte, _))| CharIndex {
			first_byte,
			char_index,
		})
		.find(|idx| idx.first_byte >= byte_index)
}

fn byte_span_to_char_span(source: &str, mut span: Range<usize>) -> Option<Range<usize>> {
	if span.start < span.end {
		std::mem::swap(&mut span.start, &mut span.end);
	}

	let start = byte_index_to_char_index(source, span.start)?;
	let end = byte_index_to_char_index(&source[start.first_byte..], span.end - span.start)? + start;
	Some(start.char_index..end.char_index)
}

impl std::fmt::Display for SourceErrorsWithSource {
	fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		use ariadne::{Config, Label, Report};

		struct SourceCache(ariadne::Source);

		impl ariadne::Cache<()> for SourceCache {
			fn fetch(&mut self, _id: &()) -> Result<&ariadne::Source, Box<dyn std::fmt::Debug + '_>> {
				Ok(&self.0)
			}

			fn display<'a>(&self, _id: &'a ()) -> Option<Box<dyn std::fmt::Display + 'a>> {
				Some(Box::new(FILE_NAME))
			}
		}

		let source_text = self.source.text();
		let mut cache = SourceCache(ariadne::Source::from(source_text));

		let mut bytes = Vec::new();

		for error in &self.errors {
			bytes.clear();

			let span = self.source.range(error.span);
			let span = match error.pos {
				ErrorPos::Full => span,
				ErrorPos::Start => span.start..span.start,
				ErrorPos::End => span.end..span.end,
			};
			let span = byte_span_to_char_span(source_text, span).ok_or(std::fmt::Error)?;

			let report = Report::build(ariadne::ReportKind::Error, (), span.start)
				.with_config(Config::default().with_tab_width(2).with_color(false))
				.with_message(&error.message)
				.with_label(Label::new(span))
				.finish();
			// The unwrap will never fail since `Vec`'s `Write` implementation is infallible.
			report.write(&mut cache, &mut bytes).unwrap();

			// The unwrap will never fail since the output string is always valid UTF-8.
			formatter.write_str(std::str::from_utf8(&bytes).unwrap())?;
		}

		Ok(())
	}
}

impl std::error::Error for SourceErrorsWithSource {}

#[derive(Debug, thiserror::Error)]
pub enum Error {
	#[error(transparent)]
	Source(#[from] SourceErrorsWithSource),
	#[error(transparent)]
	TooBig(#[from] TooBig),
	#[error("no pages in rendered output")]
	NoPages,
}

pub struct Output {
	pub image: Vec<u8>,
	pub more_pages: Option<NonZeroUsize>,
}

pub fn render(sandbox: Arc<Sandbox>, fill: Color, source: String) -> Result<Output, Error> {
	let world = sandbox.with_source(source);

	let document = typst::compile(&world).map_err(|errors| SourceErrorsWithSource {
		source: world.into_source(),
		errors: *errors,
	})?;
	let frame = &document.pages.get(0).ok_or(Error::NoPages)?;
	let more_pages = NonZeroUsize::new(document.pages.len().saturating_sub(1));

	let pixels_per_point = determine_pixels_per_point(frame.size())?;

	let pixmap = typst::export::render(frame, pixels_per_point, fill);

	let mut writer = Cursor::new(Vec::new());

	// The unwrap will never fail since `Vec`'s `Write` implementation is infallible.
	image::write_buffer_with_format(
		&mut writer,
		bytemuck::cast_slice(pixmap.pixels()),
		pixmap.width(),
		pixmap.height(),
		image::ColorType::Rgba8,
		image::ImageFormat::Png,
	)
	.unwrap();

	let image = writer.into_inner();
	Ok(Output { image, more_pages })
}
