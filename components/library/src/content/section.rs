use std::collections::HashMap;
use std::path::{Path, PathBuf};

use tera::{Tera, Context as TeraContext, Value};
use slotmap::{Key};

use config::Config;
use front_matter::{SectionFrontMatter, split_section_content};
use errors::{Result, ResultExt};
use utils::fs::{read_file, find_related_assets};
use utils::templates::render_template;
use utils::site::get_reading_analytics;
use rendering::{RenderContext, Header, render_content};

use content::file_info::FileInfo;
use content::SerializingPage;
use library::Library;


#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SerializingSection<'a> {
    content: &'a str,
    permalink: &'a str,
    title: &'a Option<String>,
    description: &'a Option<String>,
    extra: &'a HashMap<String, Value>,
    path: &'a str,
    components: &'a [String],
    word_count: Option<usize>,
    reading_time: Option<usize>,
    toc: &'a [Header],
    assets: Vec<String>,
    pages: Vec<SerializingPage<'a>>,
    subsections: Vec<SerializingSection<'a>>,
}

impl<'a> SerializingSection<'a> {
    pub fn from_section(section: &'a Section, library: &'a Library) -> Self {
        let mut pages = Vec::with_capacity(section.pages.len());
        let mut subsections = Vec::with_capacity(section.subsections.len());

        for k in &section.pages {
            pages.push(library.get_page_by_key(*k).to_serialized(library.pages()));
        }

        for k in &section.subsections {
            subsections.push(library.get_section_by_key(*k).to_serialized(library));
        }

        SerializingSection {
            content: &section.content,
            permalink: &section.permalink,
            title: &section.meta.title,
            description: &section.meta.description,
            extra: &section.meta.extra,
            path: &section.path,
            components: &section.components,
            word_count: section.word_count,
            reading_time: section.reading_time,
            toc: &section.toc,
            assets: section.serialize_assets(),
            pages,
            subsections,
        }
    }

    /// Same as from_section but doesn't fetch pages and sections
    pub fn from_section_basic(section: &'a Section) -> Self {
        SerializingSection {
            content: &section.content,
            permalink: &section.permalink,
            title: &section.meta.title,
            description: &section.meta.description,
            extra: &section.meta.extra,
            path: &section.path,
            components: &section.components,
            word_count: section.word_count,
            reading_time: section.reading_time,
            toc: &section.toc,
            assets: section.serialize_assets(),
            pages: vec![],
            subsections: vec![],
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Section {
    /// All info about the actual file
    pub file: FileInfo,
    /// The front matter meta-data
    pub meta: SectionFrontMatter,
    /// The URL path of the page
    pub path: String,
    /// The components for the path of that page
    pub components: Vec<String>,
    /// The full URL for that page
    pub permalink: String,
    /// The actual content of the page, in markdown
    pub raw_content: String,
    /// The HTML rendered of the page
    pub content: String,
    /// All the non-md files we found next to the .md file
    pub assets: Vec<PathBuf>,
    /// All direct pages of that section
    pub pages: Vec<Key>,
    /// All pages that cannot be sorted in this section
    pub ignored_pages: Vec<Key>,
    /// All direct subsections
    pub subsections: Vec<Key>,
    /// Toc made from the headers of the markdown file
    pub toc: Vec<Header>,
    /// How many words in the raw content
    pub word_count: Option<usize>,
    /// How long would it take to read the raw content.
    /// See `get_reading_analytics` on how it is calculated
    pub reading_time: Option<usize>,
}

impl Section {
    pub fn new<P: AsRef<Path>>(file_path: P, meta: SectionFrontMatter) -> Section {
        let file_path = file_path.as_ref();

        Section {
            file: FileInfo::new_section(file_path),
            meta,
            path: "".to_string(),
            components: vec![],
            permalink: "".to_string(),
            raw_content: "".to_string(),
            assets: vec![],
            content: "".to_string(),
            pages: vec![],
            ignored_pages: vec![],
            subsections: vec![],
            toc: vec![],
            word_count: None,
            reading_time: None,
        }
    }

    pub fn parse(file_path: &Path, content: &str, config: &Config) -> Result<Section> {
        let (meta, content) = split_section_content(file_path, content)?;
        let mut section = Section::new(file_path, meta);
        section.raw_content = content.clone();
        let (word_count, reading_time) = get_reading_analytics(&section.raw_content);
        section.word_count = Some(word_count);
        section.reading_time = Some(reading_time);
        section.path = format!("{}/", section.file.components.join("/"));
        section.components = section.path.split('/')
            .map(|p| p.to_string())
            .filter(|p| !p.is_empty())
            .collect::<Vec<_>>();
        section.permalink = config.make_permalink(&section.path);
        Ok(section)
    }

    /// Read and parse a .md file into a Page struct
    pub fn from_file<P: AsRef<Path>>(path: P, config: &Config) -> Result<Section> {
        let path = path.as_ref();
        let content = read_file(path)?;
        let mut section = Section::parse(path, &content, config)?;

        let parent_dir = path.parent().unwrap();
        let assets = find_related_assets(parent_dir);

        if let Some(ref globset) = config.ignored_content_globset {
            // `find_related_assets` only scans the immediate directory (it is not recursive) so our
            // filtering only needs to work against the file_name component, not the full suffix. If
            // `find_related_assets` was changed to also return files in subdirectories, we could
            // use `PathBuf.strip_prefix` to remove the parent directory and then glob-filter
            // against the remaining path. Note that the current behaviour effectively means that
            // the `ignored_content` setting in the config file is limited to single-file glob
            // patterns (no "**" patterns).
            section.assets = assets.into_iter()
                .filter(|path|
                    match path.file_name() {
                        None => true,
                        Some(file) => !globset.is_match(file)
                    }
                ).collect();
        } else {
            section.assets = assets;
        }

        Ok(section)
    }

    pub fn get_template_name(&self) -> String {
        match self.meta.template {
            Some(ref l) => l.to_string(),
            None => {
                if self.is_index() {
                    return "index.html".to_string();
                }
                "section.html".to_string()
            }
        }
    }

    /// We need access to all pages url to render links relative to content
    /// so that can't happen at the same time as parsing
    pub fn render_markdown(
        &mut self,
        permalinks: &HashMap<String, String>,
        tera: &Tera,
        config: &Config,
    ) -> Result<()> {
        let mut context = RenderContext::new(
            tera,
            config,
            &self.permalink,
            permalinks,
            self.meta.insert_anchor_links,
        );

        context.tera_context.insert("section", &SerializingSection::from_section_basic(self));

        let res = render_content(&self.raw_content, &context)
            .chain_err(|| format!("Failed to render content of {}", self.file.path.display()))?;
        self.content = res.body;
        self.toc = res.toc;
        Ok(())
    }

    /// Renders the page using the default layout, unless specified in front-matter
    pub fn render_html(&self, tera: &Tera, config: &Config, library: &Library) -> Result<String> {
        let tpl_name = self.get_template_name();

        let mut context = TeraContext::new();
        context.insert("config", config);
        context.insert("current_url", &self.permalink);
        context.insert("current_path", &self.path);
        context.insert("section", &self.to_serialized(library));

        render_template(&tpl_name, tera, &context, &config.theme)
            .chain_err(|| format!("Failed to render section '{}'", self.file.path.display()))
    }

    /// Is this the index section?
    pub fn is_index(&self) -> bool {
        self.file.components.is_empty()
    }

    /// Creates a vectors of asset URLs.
    fn serialize_assets(&self) -> Vec<String> {
        self.assets.iter()
            .filter_map(|asset| asset.file_name())
            .filter_map(|filename| filename.to_str())
            .map(|filename| self.path.clone() + filename)
            .collect()
    }

    pub fn to_serialized<'a>(&'a self, library: &'a Library) -> SerializingSection<'a> {
        SerializingSection::from_section(self, library)
    }
}

/// Used to create a default index section if there is no _index.md in the root content directory
impl Default for Section {
    fn default() -> Section {
        Section {
            file: FileInfo::default(),
            meta: SectionFrontMatter::default(),
            path: "".to_string(),
            components: vec![],
            permalink: "".to_string(),
            raw_content: "".to_string(),
            assets: vec![],
            content: "".to_string(),
            pages: vec![],
            ignored_pages: vec![],
            subsections: vec![],
            toc: vec![],
            reading_time: None,
            word_count: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::fs::{File, create_dir};

    use tempfile::tempdir;
    use globset::{Glob, GlobSetBuilder};

    use config::Config;
    use super::Section;

    #[test]
    fn section_with_assets_gets_right_info() {
        let tmp_dir = tempdir().expect("create temp dir");
        let path = tmp_dir.path();
        create_dir(&path.join("content")).expect("create content temp dir");
        create_dir(&path.join("content").join("posts")).expect("create posts temp dir");
        let nested_path = path.join("content").join("posts").join("with-assets");
        create_dir(&nested_path).expect("create nested temp dir");
        let mut f = File::create(nested_path.join("_index.md")).unwrap();
        f.write_all(b"+++\n+++\n").unwrap();
        File::create(nested_path.join("example.js")).unwrap();
        File::create(nested_path.join("graph.jpg")).unwrap();
        File::create(nested_path.join("fail.png")).unwrap();

        let res = Section::from_file(
            nested_path.join("_index.md").as_path(),
            &Config::default(),
        );
        assert!(res.is_ok());
        let section = res.unwrap();
        assert_eq!(section.assets.len(), 3);
        assert_eq!(section.permalink, "http://a-website.com/posts/with-assets/");
    }

    #[test]
    fn section_with_ignored_assets_filters_out_correct_files() {
        let tmp_dir = tempdir().expect("create temp dir");
        let path = tmp_dir.path();
        create_dir(&path.join("content")).expect("create content temp dir");
        create_dir(&path.join("content").join("posts")).expect("create posts temp dir");
        let nested_path = path.join("content").join("posts").join("with-assets");
        create_dir(&nested_path).expect("create nested temp dir");
        let mut f = File::create(nested_path.join("_index.md")).unwrap();
        f.write_all(b"+++\nslug=\"hey\"\n+++\n").unwrap();
        File::create(nested_path.join("example.js")).unwrap();
        File::create(nested_path.join("graph.jpg")).unwrap();
        File::create(nested_path.join("fail.png")).unwrap();

        let mut gsb = GlobSetBuilder::new();
        gsb.add(Glob::new("*.{js,png}").unwrap());
        let mut config = Config::default();
        config.ignored_content_globset = Some(gsb.build().unwrap());

        let res = Section::from_file(
            nested_path.join("_index.md").as_path(),
            &config,
        );

        assert!(res.is_ok());
        let page = res.unwrap();
        assert_eq!(page.assets.len(), 1);
        assert_eq!(page.assets[0].file_name().unwrap().to_str(), Some("graph.jpg"));
    }
}
