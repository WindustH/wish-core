//! Session snapshot access for independently parsed reader tools.
use super::*;
impl SessionTools {
  pub(super) async fn execute_jina_fetch(
    &self,
    request: crate::extensions::jina::fetch_tool::Request,
    services: &[Value],
    control: &ExecutionControl,
  ) -> ToolOutcome {
    let allowed: Vec<_> = services.iter().filter_map(|s| s["id"].as_str()).collect();
    let provider = if request.cursor.as_deref().is_some_and(|v| !v.trim().is_empty()) {
      request.service.as_deref()
    } else {
      request.service.as_deref().or_else(|| allowed.first().copied())
    };
    if allowed.is_empty() || provider.is_some_and(|p| !allowed.contains(&p)) {
      return ToolOutcome::Failed("no enabled service for this extension reader".into());
    }
    match control
      .run_until_cancelled(self.web_fetch.read(
        request.url.as_deref(),
        request.cursor.as_deref(),
        request.refresh.unwrap_or(false),
        provider,
        &allowed,
        crate::extensions::jina::fetch_tool::REGISTRATION.extension,
      ))
      .await
    {
      Some(Ok(page)) => ToolOutcome::Report {
        text: crate::application::pages::render(&page.output, "jina_fetch"),
        output: page.output,
        metadata: page.metadata,
        failed: false,
      },
      Some(Err(error)) => ToolOutcome::Failed(error),
      None => ToolOutcome::Cancelled,
    }
  }
}
impl SessionTools {
  pub(super) async fn execute_deepseek_fetch(
    &self,
    request: crate::extensions::deepseek::fetch_tool::Request,
    services: &[Value],
    control: &ExecutionControl,
  ) -> ToolOutcome {
    let allowed: Vec<_> = services.iter().filter_map(|s| s["id"].as_str()).collect();
    let provider = if request.cursor.as_deref().is_some_and(|v| !v.trim().is_empty()) {
      request.service.as_deref()
    } else {
      request.service.as_deref().or_else(|| allowed.first().copied())
    };
    if allowed.is_empty() || provider.is_some_and(|p| !allowed.contains(&p)) {
      return ToolOutcome::Failed("no enabled service for this extension reader".into());
    }
    match control
      .run_until_cancelled(self.web_fetch.read(
        request.url.as_deref(),
        request.cursor.as_deref(),
        request.refresh.unwrap_or(false),
        provider,
        &allowed,
        crate::extensions::deepseek::fetch_tool::REGISTRATION.extension,
      ))
      .await
    {
      Some(Ok(page)) => ToolOutcome::Report {
        text: crate::application::pages::render(&page.output, "deepseek_fetch"),
        output: page.output,
        metadata: page.metadata,
        failed: false,
      },
      Some(Err(error)) => ToolOutcome::Failed(error),
      None => ToolOutcome::Cancelled,
    }
  }
}
impl SessionTools {
  pub(super) async fn execute_openai_fetch(
    &self,
    request: crate::extensions::openai::fetch_tool::Request,
    services: &[Value],
    control: &ExecutionControl,
  ) -> ToolOutcome {
    let allowed: Vec<_> = services.iter().filter_map(|s| s["id"].as_str()).collect();
    let provider = if request.cursor.as_deref().is_some_and(|v| !v.trim().is_empty()) {
      request.service.as_deref()
    } else {
      request.service.as_deref().or_else(|| allowed.first().copied())
    };
    if allowed.is_empty() || provider.is_some_and(|p| !allowed.contains(&p)) {
      return ToolOutcome::Failed("no enabled service for this extension reader".into());
    }
    match control
      .run_until_cancelled(self.web_fetch.read(
        request.url.as_deref(),
        request.cursor.as_deref(),
        request.refresh.unwrap_or(false),
        provider,
        &allowed,
        crate::extensions::openai::fetch_tool::REGISTRATION.extension,
      ))
      .await
    {
      Some(Ok(page)) => ToolOutcome::Report {
        text: crate::application::pages::render(&page.output, "openai_fetch"),
        output: page.output,
        metadata: page.metadata,
        failed: false,
      },
      Some(Err(error)) => ToolOutcome::Failed(error),
      None => ToolOutcome::Cancelled,
    }
  }
}
impl SessionTools {
  pub(super) async fn execute_zai_fetch(
    &self,
    request: crate::extensions::zai::fetch_tool::Request,
    services: &[Value],
    control: &ExecutionControl,
  ) -> ToolOutcome {
    let allowed: Vec<_> = services.iter().filter_map(|s| s["id"].as_str()).collect();
    let provider = if request.cursor.as_deref().is_some_and(|v| !v.trim().is_empty()) {
      request.service.as_deref()
    } else {
      request.service.as_deref().or_else(|| allowed.first().copied())
    };
    if allowed.is_empty() || provider.is_some_and(|p| !allowed.contains(&p)) {
      return ToolOutcome::Failed("no enabled service for this extension reader".into());
    }
    match control
      .run_until_cancelled(self.web_fetch.read(
        request.url.as_deref(),
        request.cursor.as_deref(),
        request.refresh.unwrap_or(false),
        provider,
        &allowed,
        crate::extensions::zai::fetch_tool::REGISTRATION.extension,
      ))
      .await
    {
      Some(Ok(page)) => ToolOutcome::Report {
        text: crate::application::pages::render(&page.output, "zai_fetch"),
        output: page.output,
        metadata: page.metadata,
        failed: false,
      },
      Some(Err(error)) => ToolOutcome::Failed(error),
      None => ToolOutcome::Cancelled,
    }
  }
}
